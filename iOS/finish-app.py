"""Finish an xtool-built .app for the App Store, as Xcode would: DT* keys and the icon
catalog's keys in Info.plist, and the SDK version in the binary's LC_BUILD_VERSION
(xtool records the deployment target there, xtool issue #351).

usage: finish-app.py <app> <iPhoneOS.sdk> <Xcode.app> <icon partial.plist>
"""
import json
import plistlib
import struct
import sys
from pathlib import Path

LC_BUILD_VERSION = 0x32
MH_MAGIC_64 = 0xFEEDFACF


def xcode_number(version):
    # "27.2" -> "2720", "16.4.1" -> "1641"
    parts = [int(p) for p in version.split(".")] + [0, 0]
    return f"{parts[0]:02d}{parts[1]}{parts[2]}"


def encode_version(version):
    parts = [int(p) for p in version.split(".")] + [0, 0]
    return parts[0] << 16 | parts[1] << 8 | parts[2]


def set_sdk(binary, sdk_version):
    data = bytearray(binary.read_bytes())
    magic, _, _, _, ncmds, _, _, _ = struct.unpack_from("<8I", data, 0)
    if magic != MH_MAGIC_64:
        sys.exit(f"{binary}: not a thin 64-bit Mach-O")
    off, found = 32, False
    for _ in range(ncmds):
        cmd, size = struct.unpack_from("<2I", data, off)
        if cmd == LC_BUILD_VERSION:
            struct.pack_into("<I", data, off + 16, encode_version(sdk_version))
            found = True
        off += size
    if not found:
        sys.exit(f"{binary}: no LC_BUILD_VERSION")
    binary.write_bytes(data)


def main():
    app, sdk, xcode, partial = map(Path, sys.argv[1:5])
    settings = json.loads((sdk / "SDKSettings.json").read_text())
    system = plistlib.loads((sdk / "System/Library/CoreServices/SystemVersion.plist").read_bytes())
    xversion = plistlib.loads((xcode / "Contents/version.plist").read_bytes())

    info_path = app / "Info.plist"
    info = plistlib.loads(info_path.read_bytes())
    info.update(plistlib.loads(partial.read_bytes()))
    info.pop("CFBundleIconFile", None)
    info.update(
        DTCompiler="com.apple.compilers.llvm.clang.1_0",
        DTPlatformName="iphoneos",
        DTPlatformVersion=settings["Version"],
        DTPlatformBuild=system["ProductBuildVersion"],
        DTSDKName=settings["CanonicalName"],
        DTSDKBuild=system["ProductBuildVersion"],
        DTXcode=xcode_number(xversion["CFBundleShortVersionString"]),
        DTXcodeBuild=xversion["ProductBuildVersion"],
    )
    info_path.write_bytes(plistlib.dumps(info, fmt=plistlib.FMT_BINARY))
    set_sdk(app / info["CFBundleExecutable"], settings["Version"])


if __name__ == "__main__":
    assert xcode_number("27.2") == "2720" and xcode_number("16.4.1") == "1641"
    assert encode_version("27.2") == 0x1B0200
    main()
