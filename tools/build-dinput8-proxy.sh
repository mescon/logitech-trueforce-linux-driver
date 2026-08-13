#!/bin/sh -e
# Build the dinput8 escape proxy with mingw-w64.
#
# dinput8-escape.dll is COMMITTED to the repository, like
# tf-range-proxy.dll and for the same reason: logi-launch stages it into
# a game's directory, and the people who need it are running Linux
# without a Windows cross compiler. If you change
# dinput8-escape-proxy.cpp, run this script and commit the rebuilt DLL
# with it, or the shipped binary and the source it claims to be drift
# apart.
#
# Requires: mingw-w64-gcc (Arch: pacman -S mingw-w64-gcc)
cd "$(dirname "$0")"
x86_64-w64-mingw32-g++ -std=c++17 -O2 -shared -static \
    -o dinput8-escape.dll dinput8-escape-proxy.cpp \
    -lole32 -loleaut32 -lws2_32 -luuid -ldinput8 -ldxguid
echo "built $(stat -c%s dinput8-escape.dll) bytes -> tools/dinput8-escape.dll"
