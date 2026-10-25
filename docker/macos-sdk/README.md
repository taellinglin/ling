# macOS SDK for the build farm (optional)

`Dockerfile.build-runner` builds the `mac` (Intel, x86_64-apple-darwin) and `macarm` (Apple
Silicon, aarch64-apple-darwin) targets only when this folder holds a macOS SDK. Apple's SDK is
not redistributable, so it is not included. Either file works:

- **Xcode_*.xip** from Apple: https://developer.apple.com/download/all/?q=xcode (free Apple ID).
  The image extracts the SDK itself with osxcross's `gen_sdk_package_pbzx.sh`, so no Mac is
  needed. Xcode 15.4 (macOS 14.5 SDK) and Xcode 27.2 (macOS 27.2 SDK) are known to work; for
  27.x the image installs Clang 21 and adds a 27.x row to osxcross's SDK table (it only lists 27.0).
- **MacOSX*.sdk.tar.xz**, packaged on a Mac with osxcross's `tools/gen_sdk_package.sh`.

This folder is its own named build context (`--build-context macos-sdk=docker/macos-sdk`; the main
context ignores it via .dockerignore) and is bind-mounted, so the multi-GB .xip is not re-sent with
every build and never ends up in the image. Then rebuild the image and turn `mac` and `macarm` on
for the toolchain:

    docker build -f Dockerfile.build-runner --build-context macos-sdk=docker/macos-sdk \
      --build-context bring-browser=../bring-browser \
      --build-context horizon-browser=../horizon-browser --build-context ling-messenger=../ling-messenger \
      -t fu-build-runner:2030.1 .
    UPDATE ling_toolchains SET platforms = 'web,lin,win,mac,macarm,lingos,rpi' WHERE version = '2030.1';

The warmup step at the end of the image build prints `WARNING: build farm warmup for 'mac' failed` (or `'macarm'`)
if the cross toolchain does not work.

Downloaded Mac builds are unsigned: on the Mac, run `xattr -dr com.apple.quarantine <app>` (or
right-click > Open) the first time.
