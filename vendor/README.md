# WebKit dependency ownership

WebKit is an independently versioned engine package maintained in
[mochiOS/libs](https://github.com/mochiOS/libs), under `engines/webkit`.
Source pinning, acquisition, preflight, platform patches and Actions builds live
there. Terra owns its browser UI and engine adapter, not the engine toolchain.
The OS build system does not fetch or build WebKit.

Existing ignored `downloads`, `src` and `build` directories are left untouched.
To reuse their source archive, explicitly pass its directory to the libs builder
with `--cache`; the builder verifies the pinned digest. Old extracted source trees
retain Terra ownership and are not silently adopted by the new builder.

The mochiOS engine/backend is still unavailable. Shell packaging remains valid;
no build command here claims to produce a working browser engine.
