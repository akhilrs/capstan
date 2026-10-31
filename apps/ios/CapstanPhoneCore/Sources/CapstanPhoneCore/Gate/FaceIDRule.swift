import Foundation

/// The one Face ID rule (plan section 1.2): once per activation, and again after more than a minute away.
/// Background refresh never asks it.
public enum FaceIDRule {
    public static let awayThreshold: TimeInterval = 60

    /// - Parameters:
    ///   - settingOn: the Face ID switch in Settings (on by default).
    ///   - lastUnlock: the last passed unlock in this process; nil means none yet, a cold start included.
    ///   - backgroundedAt: when the scene last went to the background since then; nil means it has not.
    public static func required(settingOn: Bool, lastUnlock: Date?, backgroundedAt: Date?, now: Date) -> Bool {
        guard settingOn else { return false }
        guard lastUnlock != nil else { return true }
        guard let backgroundedAt else { return false }
        return now.timeIntervalSince(backgroundedAt) > awayThreshold
    }
}
