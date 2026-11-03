# Engine build inputs

Terra owns the WebKit source, dependency locks, port patches and build recipes.
The OS build must not fetch or compile these dependencies.

The mochiOS WebKit port is not available yet. No placeholder engine is generated.
The shell build is usable for package/store testing and requires `--shell-only`.

An engine prefix supplied to the package builder must contain:

- `libwebkit.so`: the actual x86-64 ELF shared library;
- `engine.json`: `{"format":1,"target":"mochios-1","version":"..."}`;
- optional regular runtime files under `runtime/` (resources and notices).

Runtime executables and ELF libraries are rejected: they require their own
explicit binary/capability declarations before this contract can support them.

The packager rejects symlinks, special files, empty metadata and incompatible
ELF headers. These checks are packaging checks, not proof that the engine works
on mochiOS. The application still uses the disconnected adapter until engine
integration is implemented. Engine build recipes will be added here as the
actual dependencies and mochiOS port become buildable.
