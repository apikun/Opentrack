# macOS packaging (M5)

`cargo xtask installer` on macOS produces:

1. `dist/OpenTrack.app` — a self-contained bundle (CLI + GUI binaries,
   `Info.plist` with the `io.github.apikun.OpenTrack` identifier);
2. `dist/OpenTrack-<version>-aarch64.dmg` — a disk image built with
   `hdiutil` from that bundle.

## Signing & notarization (requires an Apple Developer identity)

The CI cannot sign (no identity/secret); do this locally on a Mac:

```sh
# 1. Sign the app bundle and its binaries
codesign --deep --force --options runtime \
  --sign "Developer ID Application: <NAME> (<TEAMID>)" dist/OpenTrack.app

# 2. Submit for notarization
xcrun notarytool submit dist/OpenTrack-<version>-aarch64.dmg \
  --apple-id <APPLE_ID> --team-id <TEAMID> --password <APP_PASSWORD> --wait

# 3. Staple the ticket
xcrun stapler staple dist/OpenTrack.app
```

Unsigned builds run after the user right-click → Open (Gatekeeper override),
which is the documented interim state.
