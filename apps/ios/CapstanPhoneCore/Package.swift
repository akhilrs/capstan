// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "CapstanPhoneCore",
    platforms: [.iOS(.v26), .macOS(.v14)],
    products: [
        .library(name: "CapstanPhoneCore", targets: ["CapstanPhoneCore"]),
    ],
    dependencies: [
        .package(path: "../../macos/CapstanKit"),
        .package(path: "../../macos/CapstanAppCore"),
    ],
    targets: [
        .target(
            name: "CapstanPhoneCore",
            dependencies: [
                .product(name: "CapstanKit", package: "CapstanKit"),
                .product(name: "CapstanAppCore", package: "CapstanAppCore"),
            ]),
        .testTarget(
            name: "CapstanPhoneCoreTests",
            dependencies: ["CapstanPhoneCore"],
            resources: [.copy("Keys/Fixtures")]),
    ]
)
