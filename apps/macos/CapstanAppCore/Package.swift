// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "CapstanAppCore",
    platforms: [.macOS(.v14), .iOS(.v26)],
    products: [
        .library(name: "CapstanAppCore", targets: ["CapstanAppCore"]),
    ],
    dependencies: [
        .package(path: "../CapstanKit"),
    ],
    targets: [
        .target(
            name: "CapstanAppCore",
            dependencies: [.product(name: "CapstanKit", package: "CapstanKit")],
            path: "Sources/CapstanAppCore"),
        .testTarget(
            name: "CapstanAppCoreTests",
            dependencies: ["CapstanAppCore", .product(name: "CapstanKit", package: "CapstanKit")]),
    ]
)
