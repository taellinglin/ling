# macOS SDK for the build farm (optional)

`Dockerfile.build-runner` builds the `mac` target only when a macOS SDK archive is in this folder,
for example `MacOSX13.3.sdk.tar.xz`. Apple's SDK is not redistributable, so it is not included.
Package it from Xcode on a Mac with osxcross's `tools/gen_sdk_package.sh`, put the
`MacOSX*.sdk.tar.xz` file here, rebuild the image, and add `mac` to the toolchain's platforms:

    docker build -f Dockerfile.build-runner --build-context bring-browser=../bring-browser \n      --build-context horizon-browser=../horizon-browser --build-context ling-messenger=../ling-messenger \n      -t fu-build-runner:2030.1 .
    UPDATE ling_toolchains SET platforms = 'web,lin,win,mac,lingos,rpi' WHERE version = '2030.1';
