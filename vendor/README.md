# Terra-owned WebKit build

Terra owns WebKit source acquisition, the build profile, dependency requirements
and port work. The mochiOS build system does not know how to build WebKit and
must not fetch its dependencies. Nothing here writes into an OS image or SDK.

## Pinned source and offline build

The reviewed source is WPE WebKit **2.54.2**, with the SHA-256 published in the
[official release notes](https://wpewebkit.org/release/wpewebkit-2.54.2.html).
`webkit.lock.json` pins the archive URL, version and digest. Updating this file
is an explicit source update; the script never resolves `latest`.

Run from the Terra repository:

```sh
python3 tools/webkit.py fetch --network
python3 tools/webkit.py prepare
python3 tools/webkit.py doctor --target linux
python3 tools/webkit.py configure --target linux
python3 tools/webkit.py build --target linux --jobs 2
python3 tools/webkit.py install --target linux --jobs 2
```

Only `fetch --network` permits a download. An offline build can start with the
matching archive in `vendor/downloads/`. The digest is checked before reuse and
extraction. Source extraction checks member paths, link destinations and file
types, uses Python's data extraction filter, publishes only a complete tree and
refuses to replace an unowned source directory. Python 3.12+ is required.

Defaults are `vendor/downloads/`, `vendor/src/` and `vendor/build/<target>/`.
These are ignored build data. `--cache`, `--source-root` and `--build-root` allow
moving the large working directories to a suitable volume. Use the same paths
and target options on every command. Ninja is required by this upstream WPE release. Builds default to two jobs to limit memory usage.
The 25 GiB disk budget is a local preflight policy, not an upstream minimum;
`--budget-gib` adjusts it. Extraction separately reserves 1 GiB of headroom.
Do not delete other workspace caches just to satisfy this check.

`doctor.json` records missing tools, pkg-config versions and an actual C++23
executable/shared-library link probe. The probe includes threads, atomics,
exceptions, unwind and standard-library use. Target executables are not run.
`configure.log`, `build.log` and `install.log` retain complete diagnostics.
A failed preflight/configure/build returns nonzero. A successful configure is
required before build/install. Profile, compiler environment, dependency prefix
or toolchain changes require a fresh build root rather than reusing a mixed
CMake cache. Generated CMake dependencies drive ordinary incremental rebuilds.

## Dependencies and development profile

`dependencies.json` records minimum pkg-config requirements and host tools for
the checked-in bootstrap profile. It is **not** a lock of dependency source
versions, nor a claim that dependencies have been ported to mochiOS. The
[upstream CMake definitions](https://github.com/WebKit/WebKit/blob/wpewebkit-2.54.2/Source/cmake/OptionsWPE.cmake)
and CMake configure remain authoritative, including transitive dependencies.

Linux builds may use distribution development packages or an isolated prefix
specified with `--deps-prefix`. `doctor` lists the missing module names instead
of modifying the host through a package manager. Host generators include CMake
3.20+, a C++23 compiler, Python, Perl, Ruby, Bison, Flex, gperf and GLib resource
tools, and Ninja. Linux sandbox tools are also checked; the recipe does not turn off
upstream sandboxing to obtain a successful build.

`cmake/bootstrap.cmake` builds the WPE 2.0 API and headless WPEPlatform, without
Wayland/DRM platform dependencies. It deliberately leaves optional media,
speech, spellcheck, accessibility bridge and some image/font extensions out of
this **development bootstrap**. It is not the final browser feature policy.
Certificate validation and web security are not disabled. A headless engine
build is not a ViewKit rendering/input backend.

## mochiOS cross build

```sh
python3 tools/webkit.py doctor --target mochios \
  --deps-prefix /path/to/target-dependency-sysroot \
  --toolchain /path/to/mochios-shared-toolchain.cmake
# With preflight satisfied, run configure, build and install with the same options.
```

The dependency sysroot must carry `target.json`:

```json
{"format": 1, "target": "mochios-1"}
```

This marker selects an input; it does not certify ABI compatibility. The
supplied SDK toolchain must set CMake's system name to `mochiOS`, provide a real
x86-64 cross compiler and sysroot, target headers/macros (`__mochios__` rather
than host `__linux__`), a PIC/shared-capable C/C++ runtime and final link
rules, isolate library/include/package searches with `ONLY`, and search host
programs with `NEVER`. The probe rejects a Linux toolchain passed under a mochiOS
name. It performs both final links even if the SDK uses static `try_compile`.
`PKG_CONFIG_LIBDIR` and `PKG_CONFIG_SYSROOT_DIR` refer only to the marked sysroot;
host pkg-config/include/library environment search paths are cleared.

The existing SDK's static application linker flags are not a shared-library
WebKit toolchain. Existing static libc++/libc++abi/libunwind tests do not establish
shared-library exception/unwind support. No Linux compiler output is renamed or
marked as a mochiOS engine. This repository does not add glibc or replace the OS
runtime to conceal the missing port.

Still required for a native product:

- shared/PIC-capable SDK and verified dynamic C++ runtime behavior;
- mochiOS builds of GLib/GIO, ICU, libsoup/TLS, text/image/storage and graphics
  dependencies, with recorded source versions and licenses;
- WebKit/WTF platform support plus process launch, IPC, network, storage and
  Capability sandbox integration; Linux Bubblewrap is not that integration;
- a ViewKit/WPE rendering, input, IME and accessibility backend, and Terra's
  actual `Engine` adapter;
- explicit package binary/capability declarations for WebKit helper processes
  and dependent DSOs, their SONAMEs, resources and license notices.

No mochiOS port patches are currently supplied. Upstream configure may expose
additional platform requirements after the SDK/dependencies become available.

## Development outputs versus distribution

Installation uses a private `DESTDIR` under the selected target build directory,
never `/usr` on the host. Each successful install has an `installed.json` receipt
with the configuration, destination and regular-file hashes. Installation is a
development artifact, not a signed package or proof of runtime operation. Linux
and mochiOS outputs use different directories.

Upstream WPE libraries and helper processes retain their real names/SONAMEs.
They are not renamed to `libwebkit.so`. The final `libwebkit.so` integration
artifact and runtime bundle need the mochiOS adapter/package work above. No
`engine.json` claiming a working engine is generated by this build script.

The existing Terra package importer accepts an explicitly supplied prefix with:

- `libwebkit.so`: an actual x86-64 mochiOS shared library;
- `engine.json`: `{"format":1,"target":"mochios-1","version":"..."}`;
- optional regular resource/notice files in `runtime/`.

Its header/metadata checks are packaging checks, not an OS compatibility test.
Runtime helper executables and additional ELF libraries are rejected until they
have explicit binary/capability declarations. Terra still uses the disconnected
adapter; use `tools/build.py --shell-only` for shell/package testing.

## Regression checks

```sh
python3 tools/test_webkit.py
python3 tools/test_build.py
```

These exercise source integrity, unsafe archives, offline operation, source
ownership, target isolation and failed tool propagation. The toolchain test
actually links a host executable and shared library and rejects a host CMake
configuration pretending to be the mochiOS cross toolchain. They do not constitute
a WebKit build or runtime test.
