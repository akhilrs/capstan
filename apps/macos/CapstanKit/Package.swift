// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "CapstanKit",
    platforms: [.macOS(.v14), .iOS(.v26)],
    products: [
        .library(name: "CapstanKit", targets: ["CapstanKit"]),
    ],
    targets: [
        .target(name: "CapstanKit"),
        .testTarget(name: "CapstanKitTests", dependencies: ["CapstanKit"]),
    ]
)
