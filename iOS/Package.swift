// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "IrChad",
    platforms: [
        // Keep in step with build-rust.sh's `min`.
        .iOS(.v18),
    ],
    products: [
        // An xtool project should contain exactly one library product,
        // representing the main app.
        .library(name: "IrChad", targets: ["IrChad"]),
    ],
    dependencies: [
        .package(url: "https://github.com/livekit/client-sdk-swift.git", from: "2.0.0"),
    ],
    targets: [
        .target(
            name: "IrChad",
            dependencies: [
                "chad_ffiFFI",
                .product(name: "LiveKit", package: "client-sdk-swift"),
            ],
            swiftSettings: [.swiftLanguageMode(.v5)],
            // LiveKit's @objc extensions are ObjC categories nothing links
            // to by symbol; without -ObjC they're dropped (Room addDelegate: crash).
            linkerSettings: [.unsafeFlags(["-Xlinker", "-ObjC"])]
        ),
        // chad-ffi's C header; build-rust.sh puts libchad_ffi.a in rust/.
        .target(
            name: "chad_ffiFFI",
            linkerSettings: [
                .unsafeFlags(["-L\(Context.packageDirectory)/rust/aarch64-apple-ios"]),
                .linkedLibrary("chad_ffi"),
                .linkedFramework("Security"),
                .linkedFramework("SystemConfiguration"),
            ]
        ),
    ]
)
