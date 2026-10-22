# macOS SDK for the build farm (optional)

`Dockerfile.build-runner` builds the `mac` target (x86_64-apple-darwin, which also runs on Apple
Silicon through Rosetta) only when this folder holds a macOS SDK. Apple's SDK is not
redistributable, so it is not included. Either file works:

- **Xcode_*.xip** from Apple: https://developer.apple.com/download/all/?q=xcode (free Apple ID).
  Xcode 15.4 (macOS 14.5 SDK) is known to work with osxcross. The image extracts the SDK itself
  with osxcross's `gen_sdk_package_pbzx.sh`, so no Mac is needed.
- **MacOSX*.sdk.tar.xz**, packaged on a Mac with osxcross's `tools/gen_sdk_package.sh`.

The folder is bind-mounted during the build, so the multi-GB .xip never ends up in the image.
Then rebuild the image and turn `mac` on for the toolchain:

    docker build -f Dockerfile.build-runner --build-context bring-browser=../bring-browser \
      --build-context horizon-browser=../horizon-browser --build-context ling-messenger=../ling-messenger \
      -t fu-build-runner:2030.1 .
    UPDATE ling_toolchains SET platforms = 'web,lin,win,mac,lingos,rpi' WHERE version = '2030.1';

The warmup step at the end of the image build prints `WARNING: build farm warmup for 'mac' failed`
if the cross toolchain does not work.

Downloaded Mac builds are unsigned: on the Mac, run `xattr -dr com.apple.quarantine <app>` (or
right-click > Open) the first time.
