// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "CapstanSSH",
    platforms: [.iOS(.v26), .macOS(.v14)],
    products: [
        .library(name: "CapstanSSH", targets: ["CapstanSSH"]),
    ],
    dependencies: [
        .package(path: "../CapstanPhoneCore"),
        .package(path: "../../macos/CapstanKit"),
        .package(url: "https://github.com/apple/swift-nio-ssh.git", exact: "0.15.1"),
        .package(url: "https://github.com/apple/swift-nio.git", from: "2.81.0"),
    ],
    targets: [
        .target(
            name: "CapstanSSH",
            dependencies: [
                .product(name: "CapstanPhoneCore", package: "CapstanPhoneCore"),
                .product(name: "CapstanKit", package: "CapstanKit"),
                .product(name: "NIOSSH", package: "swift-nio-ssh"),
                .product(name: "NIOCore", package: "swift-nio"),
                .product(name: "NIOPosix", package: "swift-nio"),
                .product(name: "NIOConcurrencyHelpers", package: "swift-nio"),
            ]),
        .testTarget(
            name: "CapstanSSHTests",
            dependencies: [
                "CapstanSSH",
                .product(name: "CapstanPhoneCore", package: "CapstanPhoneCore"),
                .product(name: "CapstanKit", package: "CapstanKit"),
                .product(name: "NIOSSH", package: "swift-nio-ssh"),
                .product(name: "NIOCore", package: "swift-nio"),
                .product(name: "NIOPosix", package: "swift-nio"),
            ],
            // The in-process test server bridges NIO to Foundation.Process; Swift 5 mode keeps its callbacks simple.
            swiftSettings: [.swiftLanguageMode(.v5)]),
    ]
)
