//! Pure-Rust AVI (RIFF) container support for the M3 video pipeline.
//!
//! Two halves:
//!
//! * [`AviMjpegWriter`] — a streaming MJPEG-AVI **muxer** (RIFF writer that
//!   reuses the WAV chunk discipline from RFC-0003): header, one `movi` list
//!   of `00dc` JPEG chunks, `idx1` index, with the RIFF/list size fields
//!   patched at finish. No intermediate buffering of the whole stream.
//! * [`read_avi_video`] — a minimal **demuxer** that walks the RIFF tree,
//!   finds the first `vids` stream and returns its frame table
//!   (file offset + byte size per `..dc` chunk) plus the stream header info.
//!
//! Scope: single video stream MJPEG AVI, the format this codebase both
//! writes and reads (RFC-0007 "simplest honest pipeline"). Nothing here
//! links to libav.

use std::io::{Read, Seek, SeekFrom, Write};
use thiserror::Error;

/// AVI/RIFF failures.
#[derive(Debug, Error)]
pub enum AviError {
    /// File-level I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not a readable AVI/RIFF container.
    #[error("not an AVI container: {0}")]
    NotAvi(String),
    /// The AVI has no video stream we can address.
    #[error("no usable video stream: {0}")]
    NoVideo(String),
    /// A size field ran past the end of the enclosing structure.
    #[error("corrupt size field: {0}")]
    Corrupt(String),
}

#[inline]
fn fourcc(b: &[u8]) -> [u8; 4] {
    [b[0], b[1], b[2], b[3]]
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// Streaming MJPEG-AVI writer.
///
/// Usage: [`AviMjpegWriter::new`], then [`AviMjpegWriter::write_jpeg_frame`]
/// per frame (each payload must be one complete JPEG), then
/// [`AviMjpegWriter::finish`] which patches the RIFF/list sizes, writes the
/// `idx1` index and flushes. Frame count is whatever you fed it — the caller
/// (render job) owns the frame-count-×-fps acceptance rule.
pub struct AviMjpegWriter<W: Write + Seek> {
    w: W,
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
    /// idx1 entries: `(fourcc, flags, size)`; offsets are rewritten after
    /// the whole movi list is on disk.
    index: Vec<([u8; 4], u32, u32)>,
    /// File offset just past the `movi` fourcc (idx1 offset base).
    movi_base: u64,
    started: bool,
    finished: bool,
    bytes: u64,
    riff_size_pos: u64,
    movi_size_pos: u64,
    hdrl_avih_frames_pos: u64,
    strh_length_pos: u64,
}

impl<W: Write + Seek> AviMjpegWriter<W> {
    /// Builds a writer for `width`×`height` frames at `fps_num/fps_den`.
    pub fn new(w: W, width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        Self {
            w,
            width,
            height,
            fps_num: fps_num.max(1),
            fps_den: fps_den.max(1),
            index: Vec::new(),
            movi_base: 0,
            started: false,
            finished: false,
            bytes: 0,
            riff_size_pos: 0,
            movi_size_pos: 0,
            hdrl_avih_frames_pos: 0,
            strh_length_pos: 0,
        }
    }

    fn put(&mut self, data: &[u8]) -> Result<(), AviError> {
        self.w.write_all(data)?;
        self.bytes += data.len() as u64;
        Ok(())
    }

    fn chunk_header(&mut self, id: &[u8; 4], size: u32) -> Result<(), AviError> {
        self.put(id)?;
        self.put(&size.to_le_bytes())
    }

    /// Prepares the fixed header (hdrl + `movi` list header). Called lazily
    /// on the first frame; sizes patched in [`Self::finish`].
    fn write_header(&mut self) -> Result<(), AviError> {
        debug_assert!(!self.started);
        self.started = true;

        let usec_per_frame: u32 =
            ((self.fps_den as f64 / self.fps_num as f64) * 1_000_000.0).round() as u32;

        // --- compute the hdrl payload ---
        let mut hdrl = Vec::new();
        // avih chunk (56 bytes payload)
        let mut avih = Vec::with_capacity(56);
        avih.extend_from_slice(&usec_per_frame.to_le_bytes());
        // dwMaxBytesPerSec: rough upper bound (uncompressed-equivalent is
        // meaningless for MJPEG; use frame size × fps as a floor value)
        let frame_bytes = (self.width * self.height * 3).max(1);
        let max_bps = ((frame_bytes as u64 * self.fps_num as u64) / self.fps_den as u64) as u32;
        avih.extend_from_slice(&max_bps.to_le_bytes());
        avih.extend_from_slice(&0u32.to_le_bytes()); // padding granularity
        avih.extend_from_slice(&0x10u32.to_le_bytes()); // AVIF_HASINDEX
        avih.extend_from_slice(&0u32.to_le_bytes()); // dwTotalFrames (patched)
        avih.extend_from_slice(&0u32.to_le_bytes()); // dwInitialFrames
        avih.extend_from_slice(&1u32.to_le_bytes()); // dwStreams
        avih.extend_from_slice(&frame_bytes.to_le_bytes()); // suggested buffer
        avih.extend_from_slice(&self.width.to_le_bytes());
        avih.extend_from_slice(&self.height.to_le_bytes());
        avih.extend_from_slice(&[0u8; 16]); // dwReserved[4]
        assert_eq!(avih.len(), 56);
        hdrl.extend_from_slice(b"avih");
        hdrl.extend_from_slice(&(avih.len() as u32).to_le_bytes());
        hdrl.extend_from_slice(&avih);

        // strl list: strh + strf
        let mut strl = Vec::new();
        let mut strh = Vec::with_capacity(56);
        strh.extend_from_slice(b"vids");
        strh.extend_from_slice(b"MJPG");
        strh.extend_from_slice(&0u32.to_le_bytes()); // flags
        strh.extend_from_slice(&0u16.to_le_bytes()); // priority
        strh.extend_from_slice(&0u16.to_le_bytes()); // language
        strh.extend_from_slice(&0u32.to_le_bytes()); // initial frames
        strh.extend_from_slice(&self.fps_den.to_le_bytes()); // scale
        strh.extend_from_slice(&self.fps_num.to_le_bytes()); // rate
        strh.extend_from_slice(&0u32.to_le_bytes()); // start
        strh.extend_from_slice(&0u32.to_le_bytes()); // length (patched)
        strh.extend_from_slice(&frame_bytes.to_le_bytes()); // suggested buffer
        strh.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // quality
        strh.extend_from_slice(&0u32.to_le_bytes()); // sample size (video=0)
        strh.extend_from_slice(&0i16.to_le_bytes()); // rcFrame.left
        strh.extend_from_slice(&0i16.to_le_bytes()); // rcFrame.top
        strh.extend_from_slice(&(self.width as i16).to_le_bytes());
        strh.extend_from_slice(&(self.height as i16).to_le_bytes());
        assert_eq!(strh.len(), 56);
        strl.extend_from_slice(b"strh");
        strl.extend_from_slice(&(strh.len() as u32).to_le_bytes());
        strl.extend_from_slice(&strh);

        let mut strf = Vec::with_capacity(40); // BITMAPINFOHEADER
        strf.extend_from_slice(&40u32.to_le_bytes());
        strf.extend_from_slice(&(self.width as i32).to_le_bytes());
        strf.extend_from_slice(&(self.height as i32).to_le_bytes());
        strf.extend_from_slice(&1u16.to_le_bytes()); // planes
        strf.extend_from_slice(&24u16.to_le_bytes()); // bpp
        strf.extend_from_slice(b"MJPG"); // biCompression
        strf.extend_from_slice(&(self.width * self.height * 3).to_le_bytes());
        strf.extend_from_slice(&[0u8; 16]); // resolution + colors
        assert_eq!(strf.len(), 40);
        strl.extend_from_slice(b"strf");
        strl.extend_from_slice(&(strf.len() as u32).to_le_bytes());
        strl.extend_from_slice(&strf);

        hdrl.extend_from_slice(b"LIST");
        hdrl.extend_from_slice(&((strl.len() + 4) as u32).to_le_bytes());
        hdrl.extend_from_slice(b"strl");
        hdrl.extend_from_slice(&strl);

        // RIFF + 'AVI ' + LIST hdrl
        self.put(b"RIFF")?;
        self.riff_size_pos = self.bytes;
        self.put(&0u32.to_le_bytes())?; // patched at finish
        self.put(b"AVI ")?;
        self.chunk_header(b"LIST", (hdrl.len() + 4) as u32)?;
        self.put(b"hdrl")?;
        self.put(&hdrl)?;
        // Positions inside the hdrl body (avih chunk starts here):
        let hdrl_body = self.bytes - hdrl.len() as u64;
        // dwTotalFrames = avih payload offset 16 (after usec/maxbps/pad/flags)
        self.hdrl_avih_frames_pos = hdrl_body + 8 + 16;
        // strh payload = hdrl_body + 64 (avih chunk) + 8 (LIST hdr)
        //   + 4 ("strl") + 8 (strh hdr); dwLength sits 32 bytes in.
        self.strh_length_pos = hdrl_body + 64 + 8 + 4 + 8 + 32;

        // movi list header; payload size patched at finish
        self.chunk_header(b"LIST", 0)?;
        self.movi_size_pos = self.bytes - 4;
        self.put(b"movi")?;
        self.movi_base = self.bytes;
        Ok(())
    }

    /// Writes one JPEG frame as a `00dc` chunk.
    pub fn write_jpeg_frame(&mut self, jpeg: &[u8]) -> Result<(), AviError> {
        if self.finished {
            return Err(AviError::Corrupt("writer already finished".into()));
        }
        if !self.started {
            self.write_header()?;
        }
        let size = jpeg.len() as u32;
        self.chunk_header(b"00dc", size)?;
        self.put(jpeg)?;
        // Even-align chunks (RIFF convention; ffmpeg pads too).
        if jpeg.len() % 2 == 1 {
            self.put(&[0u8])?;
        }
        self.index.push((*b"00dc", 0x10, size)); // AVIIF_KEYFRAME
        Ok(())
    }

    /// Patches the header sizes, writes `idx1` and returns the frame count.
    ///
    /// Order matters: `idx1` is written sequentially at the end first, then
    /// every patch is an explicit seek+write — sequential writes never
    /// happen after a seek moved the cursor.
    pub fn finish(mut self) -> Result<u64, AviError> {
        if !self.started {
            self.write_header()?;
        }
        let frames = self.index.len() as u64;
        let movi_end = self.bytes;

        // idx1 (sequential write at the current end-of-file cursor).
        let idx_pos = self.bytes;
        self.chunk_header(b"idx1", (self.index.len() * 16) as u32)?;
        let entries: Vec<([u8; 4], u32, u32)> = self.index.clone();
        for (id, flags, size) in &entries {
            let mut entry = Vec::with_capacity(16);
            entry.extend_from_slice(id);
            entry.extend_from_slice(&flags.to_le_bytes());
            entry.extend_from_slice(&0u32.to_le_bytes()); // patched below
            entry.extend_from_slice(&size.to_le_bytes());
            self.put(&entry)?;
        }
        let total_bytes = self.bytes;

        // Patch 1: movi LIST size (size field covers 'movi' + all chunks).
        let movi_size = (movi_end - self.movi_size_pos - 4) as u32;
        self.w.seek(SeekFrom::Start(self.movi_size_pos))?;
        self.w.write_all(&movi_size.to_le_bytes())?;

        // Patch 2: idx1 entry offsets (relative to the movi data start).
        let sizes: Vec<u32> = entries.iter().map(|(_, _, sz)| *sz).collect();
        let mut off = 0u64;
        let mut p = idx_pos + 8;
        for size in &sizes {
            self.w.seek(SeekFrom::Start(p + 8))?;
            self.w.write_all(&off.to_le_bytes())?;
            off += *size as u64 + 8 + u64::from(size % 2);
            p += 16;
        }

        // Patch 3: dwTotalFrames (avih) + dwLength (strh).
        self.w.seek(SeekFrom::Start(self.hdrl_avih_frames_pos))?;
        self.w.write_all(&(frames as u32).to_le_bytes())?;
        self.w.seek(SeekFrom::Start(self.strh_length_pos))?;
        self.w.write_all(&(frames as u32).to_le_bytes())?;

        // Patch 4: RIFF size = everything after the size field.
        let riff_size = (total_bytes - 8) as u32;
        self.w.seek(SeekFrom::Start(self.riff_size_pos))?;
        self.w.write_all(&riff_size.to_le_bytes())?;

        self.w.flush()?;
        self.finished = true;
        Ok(frames)
    }
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// Video stream facts pulled from the AVI header.
#[derive(Clone, Debug, PartialEq)]
pub struct AviVideoInfo {
    /// Frame width from the stream header.
    pub width: u32,
    /// Frame height from the stream header.
    pub height: u32,
    /// FPS numerator (`dwRate`).
    pub fps_num: u32,
    /// FPS denominator (`dwScale`).
    pub fps_den: u32,
    /// Codec fourcc from `strh` (e.g. `MJPG`).
    pub codec: [u8; 4],
    /// Frame count (from `idx1` when present, else `movi` chunk count).
    pub frame_count: usize,
}

/// One video frame's location in the file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameRef {
    /// Absolute file offset of the chunk payload.
    pub offset: u64,
    /// Payload size in bytes.
    pub size: u32,
}

/// Walks an AVI file and returns header info + the video frame table.
pub fn read_avi_video<R: Read + Seek>(
    r: &mut R,
) -> Result<(AviVideoInfo, Vec<FrameRef>), AviError> {
    let mut root = [0u8; 12];
    r.read_exact(&mut root)?;
    if &root[0..4] != b"RIFF" || &root[8..12] != b"AVI " {
        return Err(AviError::NotAvi("missing RIFF/AVI ".into()));
    }
    let riff_end = 8 + u32::from_le_bytes(root[4..8].try_into().unwrap()) as u64;

    let mut width = 0u32;
    let mut height = 0u32;
    let mut fps_num = 0u32;
    let mut fps_den = 0u32;
    let mut codec = [0u8; 4];
    let mut found_video_stream = false;

    let mut frames: Vec<FrameRef> = Vec::new();
    let mut in_movi = false;
    let mut movi_end = riff_end;
    let mut movi_data_base = 0u64;
    let mut idx_relative: Vec<FrameRef> = Vec::new();
    let mut pos = 12u64;

    // Walk the top-level chunks.
    while pos + 8 <= riff_end {
        r.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 8];
        r.read_exact(&mut hdr)?;
        let id = fourcc(&hdr[0..4]);
        let size = u32::from_le_bytes(hdr[4..8].try_into().unwrap()) as u64;
        let body = pos + 8;
        match &id {
            b"LIST" => {
                let mut kind = [0u8; 4];
                r.read_exact(&mut kind)?;
                if &kind == b"hdrl" {
                    parse_hdrl(
                        r,
                        body + 4,
                        body + size,
                        &mut width,
                        &mut height,
                        &mut fps_num,
                        &mut fps_den,
                        &mut codec,
                        &mut found_video_stream,
                    )?;
                } else if &kind == b"movi" {
                    in_movi = true;
                    movi_end = body + size;
                    movi_data_base = body + 4;
                    // Walk frames inside movi below.
                }
            }
            b"idx1" => {
                // Kept as a fallback (converted to absolute offsets) — the
                // movi walk is authoritative; some writers pad/omit idx1.
                let n = (size / 16) as usize;
                let mut entry = [0u8; 16];
                let mut idx_frames = Vec::new();
                for _ in 0..n {
                    r.read_exact(&mut entry)?;
                    let chunk_id = fourcc(&entry[0..4]);
                    let off = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as u64;
                    let sz = u32::from_le_bytes(entry[12..16].try_into().unwrap());
                    if &chunk_id[2..4] == b"dc" {
                        idx_frames.push(FrameRef {
                            offset: movi_data_base + off,
                            size: sz,
                        });
                    }
                }
                if !idx_frames.is_empty() {
                    idx_relative = idx_frames;
                }
            }
            _ => {}
        }

        if in_movi {
            // Sequentially walk the movi children now (chunk by chunk).
            let mut p = body + 4; // skip 'movi'
            while p + 8 <= movi_end.min(riff_end) {
                r.seek(SeekFrom::Start(p))?;
                let mut ch = [0u8; 8];
                r.read_exact(&mut ch)?;
                let cid = fourcc(&ch[0..4]);
                let csz = u32::from_le_bytes(ch[4..8].try_into().unwrap()) as u64;
                if &cid[2..4] == b"dc" {
                    frames.push(FrameRef {
                        offset: p + 8,
                        size: csz as u32,
                    });
                }
                p += 8 + csz + (csz & 1);
            }
            in_movi = false;
            // movi is typically the last list; continue the outer walk for
            // idx1 which follows it.
        }

        pos = body + size + (size & 1);
    }

    if !found_video_stream {
        return Err(AviError::NoVideo("no vids stream header".into()));
    }
    if frames.is_empty() {
        // Some writers put everything in idx1; fall back to it.
        frames = idx_relative;
    }
    if frames.is_empty() {
        return Err(AviError::NoVideo("no frame chunks found".into()));
    }
    let info = AviVideoInfo {
        width,
        height,
        fps_num: fps_num.max(1),
        fps_den: fps_den.max(1),
        codec,
        frame_count: frames.len(),
    };
    Ok((info, frames))
}

#[allow(clippy::too_many_arguments)]
fn parse_hdrl<R: Read + Seek>(
    r: &mut R,
    start: u64,
    end: u64,
    width: &mut u32,
    height: &mut u32,
    fps_num: &mut u32,
    fps_den: &mut u32,
    codec: &mut [u8; 4],
    found_video: &mut bool,
) -> Result<(), AviError> {
    let mut pos = start;
    while pos + 8 <= end {
        r.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 8];
        r.read_exact(&mut hdr)?;
        let id = fourcc(&hdr[0..4]);
        let size = u32::from_le_bytes(hdr[4..8].try_into().unwrap()) as u64;
        let body = pos + 8;
        match &id {
            b"strh" => {
                let mut buf = vec![0u8; size as usize];
                r.read_exact(&mut buf)?;
                if buf.len() >= 56 && &buf[0..4] == b"vids" && !*found_video {
                    *found_video = true;
                    codec.copy_from_slice(&buf[4..8]);
                    *fps_den = u32::from_le_bytes(buf[20..24].try_into().unwrap()).max(1);
                    *fps_num = u32::from_le_bytes(buf[24..28].try_into().unwrap());
                }
            }
            b"strf" => {
                let mut buf = vec![0u8; size as usize];
                r.read_exact(&mut buf)?;
                if *found_video && buf.len() >= 40 && *width == 0 {
                    // BITMAPINFOHEADER: width at 4, height at 8
                    *width = i32::from_le_bytes(buf[4..8].try_into().unwrap()).unsigned_abs();
                    *height = i32::from_le_bytes(buf[8..12].try_into().unwrap()).unsigned_abs();
                }
            }
            b"LIST" => {
                // nested stream list — recurse
                let mut kind = [0u8; 4];
                r.read_exact(&mut kind)?;
                if &kind == b"strl" {
                    parse_hdrl(
                        r,
                        body + 4,
                        (body + size).min(end),
                        width,
                        height,
                        fps_num,
                        fps_den,
                        codec,
                        found_video,
                    )?;
                }
            }
            _ => {}
        }
        pos = body + size + (size & 1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ot_avi_{}_{}", name, std::process::id()))
    }

    #[test]
    fn mux_roundtrip_frame_table() {
        let path = tmp("mux.avi");
        let f = std::fs::File::create(&path).unwrap();
        let mut w = AviMjpegWriter::new(f, 64, 48, 30, 1);
        for i in 0..7 {
            // fake "JPEG" payloads of distinct sizes
            let payload = vec![i as u8; 100 + i * 7];
            w.write_jpeg_frame(&payload).unwrap();
        }
        let frames = w.finish().unwrap();
        assert_eq!(frames, 7);

        let mut r = std::fs::File::open(&path).unwrap();
        let (info, table) = read_avi_video(&mut r).unwrap();
        assert_eq!(info.width, 64);
        assert_eq!(info.height, 48);
        assert_eq!((info.fps_num, info.fps_den), (30, 1));
        assert_eq!(info.codec, *b"MJPG");
        assert_eq!(info.frame_count, 7);
        assert_eq!(table.len(), 7);
        // payload bytes survive the roundtrip
        for (i, fr) in table.iter().enumerate() {
            r.seek(SeekFrom::Start(fr.offset)).unwrap();
            let mut buf = vec![0u8; fr.size as usize];
            r.read_exact(&mut buf).unwrap();
            assert_eq!(buf, vec![i as u8; 100 + i * 7]);
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn odd_sized_chunks_stay_aligned() {
        let path = tmp("odd.avi");
        let f = std::fs::File::create(&path).unwrap();
        let mut w = AviMjpegWriter::new(f, 8, 8, 10, 1);
        w.write_jpeg_frame(&[1u8; 5]).unwrap();
        w.write_jpeg_frame(&[2u8; 4]).unwrap();
        w.finish().unwrap();
        let mut r = std::fs::File::open(&path).unwrap();
        let (info, table) = read_avi_video(&mut r).unwrap();
        assert_eq!(info.frame_count, 2);
        r.seek(SeekFrom::Start(table[0].offset)).unwrap();
        let mut b1 = vec![0u8; 5];
        r.read_exact(&mut b1).unwrap();
        assert_eq!(b1, vec![1u8; 5]);
        r.seek(SeekFrom::Start(table[1].offset)).unwrap();
        let mut b2 = vec![0u8; 4];
        r.read_exact(&mut b2).unwrap();
        assert_eq!(b2, vec![2u8; 4]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn non_avi_fails_clean() {
        let path = tmp("not.avi");
        std::fs::write(&path, b"definitely not an avi").unwrap();
        let mut r = std::fs::File::open(&path).unwrap();
        assert!(matches!(read_avi_video(&mut r), Err(AviError::NotAvi(_))));
        std::fs::remove_file(&path).ok();
    }
}
