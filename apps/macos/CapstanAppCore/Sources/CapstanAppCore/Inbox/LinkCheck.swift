import Foundation

public enum LinkCheck {
    /// The URL when the text is an http or https link with a host; nil for any other scheme or a malformed text.
    public static func openable(_ text: String) -> URL? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed.unicodeScalars.allSatisfy({ $0.value > 0x20 && $0.value != 0x7F }),
            let url = URL(string: trimmed), let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https",
            let host = url.host, !host.isEmpty
        else { return nil }
        return url
    }

    public static func isOpenable(_ text: String) -> Bool {
        openable(text) != nil
    }
}
