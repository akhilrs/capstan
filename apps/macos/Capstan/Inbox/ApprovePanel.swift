import CapstanAppCore
import CapstanKit
import SwiftUI

/// The approval part of an Operator proposal that carries `approveChoices`: Deny… and Answer… under the command,
/// the decision sheet behind Answer…, and what the host applied. The flow logic is `ApproveFlow` in CapstanAppCore.
struct ApprovePanel: View {
    let offer: ApproveOffer
    let sending: Bool
    let denyAvailable: Bool
    let answerAvailable: Bool
    /// The state of the last `op approve-as`; nil while it is being sent.
    let outcome: DecideOutcome?
    let sent: ApproveChoice?
    let hostName: String
    let onDeny: () -> Void
    let onSend: (DecideVerb) -> Void

    @State private var sheetOpen = false

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            if case .denyOnly(let limit) = offer, let limit {
                ApproveLimitView(limit: limit)
            } else if case .sheet(let flow) = offer, let limit = ApproveLimit.make(flow.choices) {
                ApproveLimitView(limit: limit)
            }
            if case .sheet = offer {
                Text(verbatim: ApproveStrings.choicesApplied).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
            HStack {
                Spacer()
                Button(action: onDeny) { Text(verbatim: ApproveStrings.denyAction) }
                    .buttonStyle(.capstanQuiet)
                    .disabled(sending || !denyAvailable)
                if case .sheet = offer {
                    Button { sheetOpen = true } label: { Text(verbatim: ApproveStrings.answerAction) }
                        .buttonStyle(.capstanPrimary)
                        .keyboardShortcut(.return, modifiers: .command)
                        .disabled(sending || !answerAvailable)
                }
            }
        }
        .sheet(isPresented: $sheetOpen) {
            if case .sheet(let flow) = offer {
                ApproveSheet(
                    flow: flow, sending: sending, outcome: outcome, sent: sent, hostName: hostName, onSend: onSend,
                    onClose: { sheetOpen = false })
            }
        }
        // The sheet closes on its own once the host applied the answer or the proposal is gone.
        .onChange(of: outcome) { _, new in
            guard sheetOpen, let sent, !sending else { return }
            switch ApprovePresentation.present(new, sent: sent, hostName: hostName) {
            case .applied: sheetOpen = false
            case .refused(let refused) where refused.closesSheet: sheetOpen = false
            default: break
            }
        }
    }
}

/// The line that says why the host holds the proposal back (grey, with the floor entry as untrusted text).
private struct ApproveLimitView: View {
    let limit: ApproveLimit

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
                Image(systemName: "info.circle")
                VStack(alignment: .leading, spacing: Tokens.space[0]) {
                    if let lead = limit.lead { Text(verbatim: lead).font(.system(size: 13, weight: .semibold)) }
                    if !limit.reason.isEmpty { Text(verbatim: limit.reason).font(.system(size: 12)) }
                }
                .fixedSize(horizontal: false, vertical: true)
            }
            if let entry = limit.floorEntry {
                UntrustedTextBlock(
                    text: UntrustedText(raw: entry, verified: true, truncated: false), label: ApproveStrings.limitedFloorEntry)
            }
        }
        .padding(Tokens.space[2])
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Tokens.color(.fill2), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
        .accessibilityElement(children: .contain)
    }
}

/// What the host applied, in the inspector above the meta: the result lines of the design.
struct ApproveResultView: View {
    let applied: ApprovePresentation.Applied

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
                Image(systemName: "checkmark").foregroundStyle(Tokens.color(.green))
                Text(verbatim: applied.lead).font(.system(size: 13, weight: .semibold))
            }
            if let prefix = applied.prefix { PrefixBox(raw: prefix) }
            if let scope = applied.scopeText { Text(verbatim: scope).font(.system(size: 13)) }
            if let window = applied.windowLine { Text(verbatim: window).font(.system(size: 13).monospacedDigit()) }
            if let ends = applied.endsLine {
                Text(verbatim: ends).font(.system(size: 12).monospacedDigit()).foregroundStyle(Tokens.color(.label2))
            }
            if let revoke = applied.revokeLine {
                Text(verbatim: revoke).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
            Text(verbatim: applied.pmLine).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
        }
        .padding(Tokens.space[2])
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Tokens.color(.green).opacity(0.12), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
        .accessibilityElement(children: .contain)
    }
}

/// A prefix as received, in its own box: mono, never trimmed or cut, wrapped inside the box when long.
private struct PrefixBox: View {
    let raw: String

    var body: some View {
        Text(verbatim: raw)
            .font(.system(size: 12, design: .monospaced))
            .foregroundStyle(Tokens.color(.label))
            .fixedSize(horizontal: false, vertical: true)
            .textSelection(.enabled)
            .padding(Tokens.space[1])
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Tokens.color(.fill2), in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Tokens.color(.sep)))
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Prefix, one value")
            .accessibilityValue(raw)
    }
}

/// The decision sheet: step 1 the answer, step 2 the scope or the window; Return is the primary, Esc cancels.
private struct ApproveSheet: View {
    @State var flow: ApproveFlow
    let sending: Bool
    let outcome: DecideOutcome?
    let sent: ApproveChoice?
    let hostName: String
    let onSend: (DecideVerb) -> Void
    let onClose: () -> Void

    @State private var confirmAuto = false
    @State private var showRefusal = false

    init(
        flow: ApproveFlow, sending: Bool, outcome: DecideOutcome?, sent: ApproveChoice?, hostName: String,
        onSend: @escaping (DecideVerb) -> Void, onClose: @escaping () -> Void
    ) {
        _flow = State(initialValue: flow)
        self.sending = sending
        self.outcome = outcome
        self.sent = sent
        self.hostName = hostName
        self.onSend = onSend
        self.onClose = onClose
    }

    private var refusal: ApprovePresentation.Refused? {
        guard showRefusal, !sending, let sent else { return nil }
        if case .refused(let r) = ApprovePresentation.present(outcome, sent: sent, hostName: hostName) { return r }
        return nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            HStack {
                Text(verbatim: title).font(.system(size: 15, weight: .semibold))
                Spacer()
                if let step = flow.stepText {
                    Text(verbatim: step).font(.system(size: 12).monospacedDigit()).foregroundStyle(Tokens.color(.label2))
                }
            }
            ScrollView { stepBody.frame(maxWidth: .infinity, alignment: .leading) }
            if let refusal { refusalView(refusal) }
            buttons
        }
        .padding(Tokens.space[3])
        .frame(width: 440, height: 480)
        .alert(ApproveStrings.autoConfirmTitle, isPresented: $confirmAuto) {
            Button(role: .cancel) {} label: { Text(verbatim: ApproveStrings.actionCancel) }
                .keyboardShortcut(.defaultAction)
            Button { send(flow.confirmedWindow()) } label: { Text(verbatim: ApproveStrings.autoConfirmAction) }
        } message: {
            Text(verbatim: ApproveStrings.autoCoverage)
        }
    }

    private var title: String {
        switch flow.step {
        case .answer: return ApproveStrings.choicesHeading
        case .scope: return ApproveStrings.scopeTitle
        case .window: return ApproveStrings.autoTitle
        }
    }

    @ViewBuilder
    private var stepBody: some View {
        switch flow.step {
        case .answer:
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                ForEach(flow.answers, id: \.self) { answer in
                    radio(
                        title: title(of: answer), detail: detail(of: answer), chosen: flow.answer == answer
                    ) { flow.choose(answer) }
                }
            }
        case .scope:
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                if flow.offersExact {
                    radio(
                        title: ApproveStrings.scopeExactTitle, detail: ApproveStrings.scopeExactDetail,
                        chosen: flow.scope == .exact
                    ) { flow.choose(scope: .exact) }
                }
                ForEach(flow.prefixes, id: \.self) { raw in
                    radio(
                        title: ApproveStrings.scopePrefixTitle, detail: ApproveStrings.scopePrefixWrapped,
                        chosen: flow.scope == .prefix(raw), value: raw
                    ) { flow.choose(scope: .prefix(raw)) }
                }
                note(ApproveStrings.scopeRule)
                note(ApproveStrings.scopeLength)
            }
        case .window:
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                ForEach(flow.windows, id: \.wireName) { window in
                    radio(
                        title: window == .hour ? ApproveStrings.autoHourTitle : ApproveStrings.autoSessionTitle,
                        detail: window == .hour ? ApproveStrings.autoHourEnd : ApproveStrings.autoSessionEnd,
                        chosen: flow.window == window
                    ) { flow.choose(window: window) }
                }
                note(ApproveStrings.autoEndBoth)
                Text(verbatim: ApproveStrings.autoCoverageHeading).font(.system(size: 13, weight: .semibold))
                note(ApproveStrings.autoCoverage)
            }
        }
    }

    private func title(of answer: ApproveAnswer) -> String {
        switch answer {
        case .once: return ApproveStrings.choiceOnceTitle
        case .similar: return ApproveStrings.choiceSimilarTitle
        case .auto: return ApproveStrings.choiceAutoTitle
        }
    }

    private func detail(of answer: ApproveAnswer) -> String {
        switch answer {
        case .once: return ApproveStrings.choiceOnceDetail
        case .similar: return ApproveStrings.choiceSimilarDetail
        case .auto: return ApproveStrings.choiceAutoDetail
        }
    }

    private func note(_ text: String) -> some View {
        Text(verbatim: text).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            .fixedSize(horizontal: false, vertical: true)
    }

    private func radio(
        title: String, detail: String, chosen: Bool, value: String? = nil, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(alignment: .top, spacing: Tokens.space[1]) {
                Image(systemName: chosen ? "largecircle.fill.circle" : "circle")
                    .foregroundStyle(chosen ? Tokens.color(.accent) : Tokens.color(.label2))
                VStack(alignment: .leading, spacing: 2) {
                    Text(verbatim: title).font(.system(size: 13, weight: .semibold))
                    if let value { PrefixBox(raw: value) }
                    Text(verbatim: detail).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
            }
            .padding(Tokens.space[1])
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                chosen ? Tokens.color(.accent).opacity(0.12) : Tokens.color(.fill),
                in: RoundedRectangle(cornerRadius: Tokens.radius.row)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Tokens.radius.row)
                    .strokeBorder(chosen ? Tokens.color(.accent) : Color.clear, lineWidth: 1.5))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(chosen ? .isSelected : [])
    }

    private func refusalView(_ refused: ApprovePresentation.Refused) -> some View {
        let tone: Tone = refused.tone == .yellow ? .yellow : refused.tone == .grey ? .grey : .red
        return VStack(alignment: .leading, spacing: Tokens.space[0]) {
            Text(verbatim: refused.text).font(.system(size: 13, weight: .semibold))
            Text(verbatim: refused.nothing + " " + refused.code).font(.system(size: 12))
                .foregroundStyle(Tokens.color(.label2))
            if refused.showsHostMessage, let message = refused.hostMessage {
                UntrustedTextBlock(text: message, label: ApproveStrings.refusalHostSays)
            }
        }
        .padding(Tokens.space[2])
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(tone.color.opacity(0.12), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
        .accessibilityElement(children: .combine)
    }

    private var primaryEnabled: Bool {
        if sending { return false }
        if let refusal { return refusal.primaryLive }
        return flow.canAdvance
    }

    private var buttons: some View {
        HStack {
            if flow.step == .answer {
                Button(action: onClose) { Text(verbatim: ApproveStrings.actionCancel) }
                    .buttonStyle(.capstanQuiet)
                    .keyboardShortcut(.cancelAction)
            } else {
                Button { flow.back(); showRefusal = false } label: { Text(verbatim: ApproveStrings.actionBack) }
                    .buttonStyle(.capstanQuiet)
                    .keyboardShortcut(.cancelAction)
            }
            Spacer()
            if sending { ProgressView().controlSize(.small) }
            Button {
                switch flow.advance() {
                case .send(let verb): send(verb)
                case .confirmAuto: confirmAuto = true
                case .moved, .disabled: break
                }
            } label: { Text(verbatim: refusal?.action ?? flow.primaryTitle) }
            .buttonStyle(.capstanPrimary)
            .keyboardShortcut(.defaultAction)
            .disabled(!primaryEnabled)
        }
    }

    private func send(_ verb: DecideVerb?) {
        guard let verb else { return }
        showRefusal = true
        onSend(verb)
    }
}
