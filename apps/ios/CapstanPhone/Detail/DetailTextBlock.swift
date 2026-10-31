import CapstanAppCore
import CapstanKit
import SwiftUI

/// What of a host text a details screen shows: nothing when the text failed its integrity check, else at most
/// `ProtocolLimits.textCap` bytes, with how much was left out.
struct ShownText: Equatable {
    /// Empty when the text is not verified.
    let text: String
    let verified: Bool
    /// The host cut the text before sending it.
    let hostCut: Bool
    /// The bytes this app did not show.
    let hiddenBytes: Int

    /// `keepLines` keeps the text's line breaks (a message); otherwise it is one run of text (a command, a prompt).
    init(_ untrusted: UntrustedText, keepLines: Bool = false) {
        verified = untrusted.verified
        guard untrusted.verified else {
            text = ""
            hostCut = false
            hiddenBytes = 0
            return
        }
        let plain = keepLines ? untrusted.displayLines : untrusted.displayPlain
        let shown = TextBudget.displayPrefix(plain, maxBytes: ProtocolLimits.textCap)
        text = shown.text
        hostCut = untrusted.truncated
        hiddenBytes = shown.cut ? max(0, plain.utf8.count - shown.text.utf8.count) : 0
    }

    var cut: Bool { verified && (hostCut || hiddenBytes > 0) }
    /// The whole text is on screen: a decision about it can be taken here.
    var whole: Bool { verified && !cut }
    /// Something can be copied.
    var any: Bool { verified && !text.isEmpty }
}

/// Host text in a details screen (boards 03 and 04): a flat mono block whose header names the text and says how it is
/// shown. It wraps and never truncates; a text that failed its check is not shown at all. Never glass, never a link.
struct DetailTextBlock: View {
    let text: UntrustedText
    /// The name of the text: "Command", "Prompt", "Message", "Screens", "Link".
    let name: String
    /// How a whole, verified text is shown, after "untrusted, ".
    var shownAs = "shown as written"
    var symbol = "exclamationmark.triangle"
    /// Keep the text's line breaks: a message from an agent laid out in lines.
    var keepLines = false

    var body: some View {
        let shown = ShownText(text, keepLines: keepLines)
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: Tokens.space[1]) {
                Image(systemName: symbol).imageScale(.small).accessibilityHidden(true)
                Text(verbatim: name + " · untrusted, " + state(shown))
                    .font(TypeStyle.footnote.weight(.semibold))
            }
            .foregroundStyle(Tokens.color(.label2))
            .padding(.horizontal, Tokens.space[2])
            .padding(.vertical, Tokens.space[1])
            .frame(maxWidth: .infinity, alignment: .leading)
            Divider()
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                if !shown.verified {
                    Text("Not shown: it failed its integrity check.")
                        .font(TypeStyle.subheadline)
                        .foregroundStyle(Tokens.color(.label2))
                } else {
                    Text(verbatim: shown.text)
                        .font(TypeStyle.mono)
                        .foregroundStyle(Tokens.color(.label))
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    if shown.hiddenBytes > 0 {
                        Text(verbatim: "[" + shown.hiddenBytes.formatted() + " more bytes not shown]")
                            .font(TypeStyle.mono)
                            .foregroundStyle(Tokens.color(.label2))
                    } else if shown.hostCut {
                        Text(verbatim: "[cut by the host]")
                            .font(TypeStyle.mono)
                            .foregroundStyle(Tokens.color(.label2))
                    }
                }
            }
            .padding(Tokens.space[2])
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Tokens.color(.fill))
        .clipShape(RoundedRectangle(cornerRadius: Tokens.radius.block))
        .overlay(RoundedRectangle(cornerRadius: Tokens.radius.block).strokeBorder(Tokens.color(.sep), lineWidth: 0.5))
        .accessibilityElement(children: .combine)
    }

    private func state(_ shown: ShownText) -> String {
        if !shown.verified { return "not shown" }
        if shown.cut { return "cut at " + String(ProtocolLimits.textCap) + " bytes" }
        return shownAs
    }
}

/// The line under a cut text.
struct CutTextNote: View {
    var body: some View {
        Text(verbatim: "Capstan shows at most " + String(ProtocolLimits.textCap) + " bytes. The whole text is on the host.")
            .font(TypeStyle.footnote)
            .foregroundStyle(Tokens.color(.label2))
    }
}

/// A tonal notice in a details screen: a glyph, a title and one sentence. Content, never glass.
struct DetailNotice: View {
    let tone: Tone
    let symbol: String
    let title: String
    let text: String

    var body: some View {
        HStack(alignment: .top, spacing: Tokens.space[2]) {
            Image(systemName: symbol)
                .foregroundStyle(tone == .grey ? Tokens.color(.label2) : tone.color)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: Tokens.space[0]) {
                Text(verbatim: title).font(TypeStyle.subheadline.weight(.semibold))
                Text(verbatim: text).font(TypeStyle.subheadline).fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(Tokens.space[3])
        .background(background, in: RoundedRectangle(cornerRadius: Tokens.radius.section))
        .accessibilityElement(children: .combine)
    }

    private var background: Color {
        tone == .grey ? Tokens.color(.fill2) : tone.color.opacity(0.14)
    }
}

#Preview("DetailTextBlock") {
    ScrollView {
        VStack(spacing: Tokens.space[3]) {
            DetailTextBlock(
                text: UntrustedText(raw: "Bash command\nrm -rf build/ && npm run build", verified: true, truncated: false),
                name: "Prompt")
            DetailTextBlock(
                text: UntrustedText(raw: String(repeating: "git commit -am format && ", count: 300), verified: true, truncated: false),
                name: "Command")
            CutTextNote()
            DetailTextBlock(text: UntrustedText(raw: "", verified: false, truncated: false), name: "Command")
            DetailNotice(
                tone: .yellow, symbol: "exclamationmark.triangle.fill", title: "Text failed its integrity check",
                text: "The command does not match its hash. Answer it in the terminal.")
        }
        .padding()
    }
    .background(Tokens.color(.ground))
}
