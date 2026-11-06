import CapstanKit
import CapstanPhoneCore
import SwiftUI

/// The decision sheet for an Operator proposal (plan-153, sheet B): Approve once; Approve + allow similar (this exact
/// command, or one offered prefix); Auto-approve all (for 1 hour or the session, after a confirm). All the steps and
/// words are in CapstanPhoneCore/Approve; this only draws them. A prefix is drawn whole, in mono, as the host offered it.
struct ApproveSheet: View {
    let plan: ApprovePlan
    /// The command as verified and shown whole.
    let command: String
    let projectName: String
    let sending: Bool
    let refusal: ApproveRefusalLines?
    let onSend: (DecideVerb) -> Void
    let onShowNew: () -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var state: ApproveSheetState

    init(
        plan: ApprovePlan, command: String, projectName: String, sending: Bool, refusal: ApproveRefusalLines?,
        onSend: @escaping (DecideVerb) -> Void, onShowNew: @escaping () -> Void
    ) {
        self.plan = plan
        self.command = command
        self.projectName = projectName
        self.sending = sending
        self.refusal = refusal
        self.onSend = onSend
        self.onShowNew = onShowNew
        _state = State(initialValue: ApproveSheetState(plan: plan))
    }

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: Tokens.space[3]) {
                    header
                    commandBlock
                    stepBody
                    if let refusal { refusalBlock(refusal) }
                }
                .padding(Tokens.margin)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            actions
        }
        .background(Tokens.color(.ground))
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .interactiveDismissDisabled(sending)
    }

    // MARK: Parts

    private var title: String {
        switch state.step {
        case .choose: return ApproveStrings.choicesHeading
        case .scope: return ApproveStrings.scopeTitle
        case .window: return ApproveStrings.autoTitle
        case .confirmAuto: return ApproveStrings.autoConfirmTitle
        }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(verbatim: title).font(TypeStyle.title2).accessibilityAddTraits(.isHeader)
            Spacer(minLength: Tokens.space[2])
            if let steps = state.stepText {
                Text(verbatim: steps).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
            }
        }
    }

    private var commandBlock: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            Text(verbatim: "Command · " + projectName)
                .font(TypeStyle.footnote.weight(.semibold))
                .foregroundStyle(Tokens.color(.label2))
            Text(verbatim: command)
                .font(TypeStyle.mono)
                .foregroundStyle(Tokens.color(.label))
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(Tokens.space[2])
        .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private var stepBody: some View {
        switch state.step {
        case .choose: chooseStep
        case .scope: scopeStep
        case .window: windowStep
        case .confirmAuto: confirmStep
        }
    }

    private var chooseStep: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            if let limit = plan.limitText {
                Text(verbatim: ApproveStrings.limitedLead).font(TypeStyle.subheadline.weight(.semibold))
                Text(verbatim: limit).font(TypeStyle.subheadline).fixedSize(horizontal: false, vertical: true)
                if let entry = plan.floorEntry, !entry.isEmpty {
                    Text(verbatim: ApproveStrings.limitedFloorEntry)
                        .font(TypeStyle.footnote.weight(.semibold))
                        .foregroundStyle(Tokens.color(.label2))
                    Text(verbatim: entry)
                        .font(TypeStyle.mono)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            ForEach(plan.tops, id: \.self) { top in
                row(
                    title: topTitle(top), detail: topDetail(top), selected: state.top == top,
                    enabled: plan.tops.count > 1
                ) { state.pick(top) }
            }
            Text(verbatim: ApproveStrings.choicesApplied)
                .font(TypeStyle.footnote)
                .foregroundStyle(Tokens.color(.label2))
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var scopeStep: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            ForEach(Array(plan.scopes.enumerated()), id: \.offset) { _, scope in
                switch scope {
                case .exact:
                    row(
                        title: ApproveStrings.scopeExactTitle, detail: ApproveStrings.scopeExactDetail,
                        selected: state.scope == scope, enabled: true
                    ) { state.pick(scope) }
                case .prefix(let raw):
                    row(
                        title: ApproveStrings.scopePrefixTitle, detail: ApproveStrings.scopePrefixWrapped,
                        mono: raw, selected: state.scope == scope, enabled: true
                    ) { state.pick(scope) }
                }
            }
            note(ApproveStrings.scopeRule)
            note(ApproveStrings.scopeLength)
        }
    }

    private var windowStep: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            ForEach(plan.windows, id: \.self) { window in
                row(
                    title: window == .hour ? ApproveStrings.autoHourTitle : ApproveStrings.autoSessionTitle,
                    detail: window == .hour ? ApproveStrings.autoHourEnd : ApproveStrings.autoSessionEnd,
                    selected: state.window == window, enabled: true
                ) { state.pick(window) }
            }
            note(ApproveStrings.autoEndBoth)
            Text(verbatim: ApproveStrings.autoCoverageHeading).font(TypeStyle.subheadline.weight(.semibold))
            Text(verbatim: ApproveStrings.autoCoverage)
                .font(TypeStyle.subheadline)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var confirmStep: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            Text(verbatim: state.window == .session ? ApproveStrings.autoSessionTitle : ApproveStrings.autoHourTitle)
                .font(TypeStyle.headline)
            note(state.window == .session ? ApproveStrings.autoSessionEnd : ApproveStrings.autoHourEnd)
            note(ApproveStrings.autoEndBoth)
            Text(verbatim: ApproveStrings.autoCoverage)
                .font(TypeStyle.subheadline)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func refusalBlock(_ refusal: ApproveRefusalLines) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            Text(verbatim: refusal.lead).font(TypeStyle.subheadline.weight(.semibold))
            Text(verbatim: refusal.text).font(TypeStyle.subheadline).fixedSize(horizontal: false, vertical: true)
            if let host = refusal.hostText {
                Text(verbatim: ApproveStrings.refusalHostSays)
                    .font(TypeStyle.footnote.weight(.semibold))
                    .foregroundStyle(Tokens.color(.label2))
                Text(verbatim: host.displayPlain).font(TypeStyle.mono).fixedSize(horizontal: false, vertical: true)
            }
            if refusal.action == .showNew {
                Button { onShowNew(); dismiss() } label: { Text(verbatim: ApproveStrings.refusalHashMismatchAction) }
                    .buttonStyle(.capstanQuiet)
            }
        }
        .padding(Tokens.space[3])
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Tokens.color(.red).opacity(0.14), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
        .accessibilityElement(children: .combine)
    }

    // MARK: Actions

    @ViewBuilder
    private var actions: some View {
        let replaced = refusal?.replacesActions == true
        VStack(spacing: Tokens.space[2]) {
            if replaced {
                Button { dismiss() } label: { Text(verbatim: ApproveStrings.actionBack) }
                    .buttonStyle(.capstanQuiet)
            } else {
                Button {
                    if state.step == .choose { dismiss() } else { state.back() }
                } label: {
                    Text(verbatim: state.step == .choose ? ApproveStrings.actionCancel : ApproveStrings.actionBack)
                }
                .buttonStyle(.capstanQuiet)
                .disabled(sending)
                Button { primary() } label: {
                    if sending { Label { Text(verbatim: ApproveStrings.resultSending) } icon: { Image(systemName: "clock") } } else { Text(verbatim: state.primaryLabel) }
                }
                .buttonStyle(.capstanPrimary)
                .disabled(sending || !state.primaryEnabled)
            }
        }
        .padding(.horizontal, Tokens.margin)
        .padding(.top, Tokens.space[2])
        .padding(.bottom, Tokens.space[1])
        .background(Tokens.color(.ground))
    }

    private func primary() {
        if let verb = state.request {
            onSend(verb)
        } else {
            _ = state.advance()
        }
    }

    // MARK: Pieces

    private func topTitle(_ top: ApproveTop) -> String {
        switch top {
        case .once: return ApproveStrings.choiceOnceTitle
        case .similar: return ApproveStrings.choiceSimilarTitle
        case .auto: return ApproveStrings.choiceAutoTitle
        }
    }

    private func topDetail(_ top: ApproveTop) -> String {
        switch top {
        case .once: return ApproveStrings.choiceOnceDetail
        case .similar: return ApproveStrings.choiceSimilarDetail
        case .auto: return ApproveStrings.choiceAutoDetail
        }
    }

    private func note(_ text: String) -> some View {
        Text(verbatim: text)
            .font(TypeStyle.footnote)
            .foregroundStyle(Tokens.color(.label2))
            .fixedSize(horizontal: false, vertical: true)
    }

    /// A selectable row. `mono` is a value drawn whole and wrapped, exactly as given.
    private func row(
        title: String, detail: String, mono: String? = nil, selected: Bool, enabled: Bool, action: @escaping () -> Void
    ) -> some View {
        Button {
            action()
            Haptics.play(.selection)
        } label: {
            HStack(alignment: .top, spacing: Tokens.space[2]) {
                VStack(alignment: .leading, spacing: Tokens.space[1]) {
                    Text(verbatim: title).font(TypeStyle.subheadline.weight(.semibold))
                    if let mono {
                        Text(verbatim: mono)
                            .font(TypeStyle.mono)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Text(verbatim: detail)
                        .font(TypeStyle.footnote)
                        .foregroundStyle(Tokens.color(.label2))
                        .fixedSize(horizontal: false, vertical: true)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                if selected {
                    Image(systemName: "checkmark.circle")
                        .foregroundStyle(Tokens.color(.accent))
                        .accessibilityHidden(true)
                }
            }
            .padding(Tokens.space[3])
            .frame(minHeight: 44)
            .background(Tokens.color(.fill2), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
            .overlay(
                RoundedRectangle(cornerRadius: Tokens.radius.section)
                    .strokeBorder(selected ? Tokens.color(.accent) : Color.clear, lineWidth: 2))
            .contentShape(RoundedRectangle(cornerRadius: Tokens.radius.section))
        }
        .buttonStyle(.plain)
        .disabled(!enabled || sending)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}
