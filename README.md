# Rashamon Arc

Native system browser for Kamelot OS.

## Architecture

```
rashamon-arc/
├── crates/
│   ├── ui/          # UI process (main binary)
│   ├── renderer/    # Servo/WPE WebKit rendering engine
│   ├── net/         # Network process + adblock
│   ├── ipc/         # Shared-memory IPC between processes
│   └── sandbox/     # Capability-based process isolation
└── config/
    └── adblock/     # Adblock rules (EasyList format)
```

## Design Principles

1. **Speed** — software rendering first, dirty-rect repaints, shared memory IPC
2. **Security** — capability sandbox, per-origin storage isolation, no direct filesystem access
3. **Low memory** — aggressive background tab throttling, minimal attack surface
4. **Native OS integration** — framebuffer-first rendering, DRM/KMS display
5. **Controlled customization** — native theming, privacy profiles, per-site permissions
6. **Built-in ad blocking** — network-level blocking in the network process

## Building

```bash
cargo build --release
```

Binary: `target/release/rashamon-arc` (currently ~356 KB stripped)

## Running

```bash
# Start with a URL
./target/release/rashamon-arc https://example.com

# Start blank
./target/release/rashamon-arc
```

Press `Escape` to quit.

## Performance Targets

| Metric | Target |
|--------|--------|
| Cold start | 150–300 ms |
| Simple page first paint | < 100 ms (cached) |
| RAM per simple tab | < 200 MB |
| Binary size | < 1 MB |

## Current Status

**MVP skeleton** — all core modules implemented as stubs:
- ✅ Multi-crate workspace structure
- ✅ Shared-memory IPC layer
- ✅ Framebuffer software rendering
- ✅ Servo integration stub (full integration pending)
- ✅ Network process with built-in adblock (23 default rules)
- ✅ Capability sandbox skeleton
- ✅ PPM frame output for verification

**Next steps:**
- Full Servo engine integration
- DRM/KMS direct display (no X11/Wayland dependency)
- evdev input handling
- Multi-process separation (spawn renderer/net as child processes)
- Tab management
- HTTPS support
- Cookie/cache system

## License

MIT
