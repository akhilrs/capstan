import CapstanAppCore
import CapstanKit
import SwiftUI

/// What the PM card's controls do. The defaults do nothing, for previews.
struct PmCardActions {
    var open: (PmCardLink) -> Void = { _ in }
    var pause: (PmCardStuck) -> Void = { _ in }
    /// False when the project cannot be paused from here (already paused, no decide, a call in flight).
    var canPause = true
}

/// The "Project manager" section of a project's overview (plan-152, design-pm, iPhone card): the header with the
/// PM's activity, the stuck banner when an episode is open, the two message previews, and Waiting on. One column.
/// The host text (the two previews) is plain mono text; the card is an opaque cell, not glass.
struct PmCardView: View {
    let card: PmCardModel
    let actions: PmCardActions

    @Environment(\.dynamicTypeSize) private var typeSize

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            heading
            VStack(alignment: .leading, spacing: 0) {
                if let noPM = card.noPMText {
                    Text(verbatim: noPM)
                        .font(TypeStyle.subheadline)
                        .foregroundStyle(Tokens.color(.label2))
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(Tokens.space[3])
                        .frame(maxWidth: .infinity, alignment: .leading)
                } else {
                    if let stuck = card.stuck {
                        PmStuckBanner(stuck: stuck, actions: actions)
                    }
                    header
                    messageRow(
                        title: "From the PM", message: card.fromPM, emptyText: card.fromPMEmptyText, delivery: nil)
                    messageRow(
                        title: "You sent", message: card.youSent, emptyText: card.youSentEmptyText,
                        delivery: card.delivery)
                    waiting
                }
            }
            .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
            .clipShape(RoundedRectangle(cornerRadius: Tokens.radius.section))
        }
    }

    // MARK: Heading and header

    private var heading: some View {
        HStack {
            Text("Project manager").font(TypeStyle.footnote.weight(.semibold)).foregroundStyle(Tokens.color(.label2))
            Spacer(minLength: Tokens.space[1])
            if case .lastKnown(let time) = card.freshness {
                Text(verbatim: "as of " + time).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
            }
        }
        .padding(.horizontal, Tokens.space[2])
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isHeader)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            if typeSize.isAccessibilitySize {
                Text(verbatim: card.agentId ?? "").font(TypeStyle.mono.weight(.semibold))
                StatePill(
                    word: card.activityWord, symbol: card.activitySymbol,
                    tone: card.health.map(AgentTone.tone) ?? .grey)
            } else {
                HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
                    Text(verbatim: card.agentId ?? "").font(TypeStyle.mono.weight(.semibold)).lineLimit(1)
                    Spacer(minLength: Tokens.space[1])
                    StatePill(
                        word: card.activityWord, symbol: card.activitySymbol,
                        tone: card.health.map(AgentTone.tone) ?? .grey)
                        .fixedSize()
                }
            }
            if let since = card.sinceText {
                Text(verbatim: since).font(TypeStyle.subheadline.monospacedDigit()).foregroundStyle(Tokens.color(.label2))
            }
            freshness
        }
        .padding(Tokens.space[3])
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private var freshness: some View {
        switch card.freshness {
        case .current:
            EmptyView()
        case .lastKnown(let time):
            StatePill(word: "Last known · " + time, symbol: "clock", tone: .grey).fixedSize()
        case .stale(let time):
            StatePill(word: "Stale · as of " + time, symbol: "exclamationmark.triangle", tone: .yellow).fixedSize()
            Text("The last refresh of this card failed. Pull down to try again.")
                .font(TypeStyle.footnote)
                .foregroundStyle(Tokens.color(.label2))
        }
    }

    // MARK: Messages

    private func messageRow(
        title: String, message: PmCardMessage?, emptyText: String, delivery: PmCardDelivery?
    ) -> some View {
        VStack(spacing: 0) {
            Divider().padding(.leading, Tokens.space[3])
            Button { actions.open(.conversation) } label: {
                HStack(alignment: .top, spacing: Tokens.space[1]) {
                    VStack(alignment: .leading, spacing: Tokens.space[0]) {
                        Text(verbatim: message.flatMap { $0.time.isEmpty ? nil : title + " · " + $0.time } ?? title)
                            .font(TypeStyle.footnote.weight(.semibold))
                            .foregroundStyle(Tokens.color(.label2))
                        if let message {
                            if let text = message.text {
                                Text(verbatim: text)
                                    .font(TypeStyle.monoSmall)
                                    .lineLimit(2)
                                    .truncationMode(.tail)
                                    .multilineTextAlignment(.leading)
                            } else {
                                Text("Text not shown: the host could not vouch for it")
                                    .font(TypeStyle.footnote)
                                    .foregroundStyle(Tokens.color(.label2))
                            }
                        } else {
                            Text(verbatim: emptyText).font(TypeStyle.footnote).foregroundStyle(Tokens.color(.label2))
                        }
                        if let delivery {
                            Label { Text(verbatim: delivery.text) } icon: { Image(systemName: delivery.symbol) }
                                .font(TypeStyle.footnote)
                                .foregroundStyle(delivery.failed ? Tokens.color(.red) : Tokens.color(.label2))
                        }
                    }
                    Spacer(minLength: 0)
                    chevron
                }
                .padding(Tokens.space[3])
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(PmRowStyle())
            .accessibilityElement(children: .combine)
            .accessibilityHint(Text("Opens the conversation"))
        }
    }

    private var chevron: some View {
        Image(systemName: "chevron.right")
            .font(TypeStyle.footnote.weight(.semibold))
            .foregroundStyle(Tokens.color(.label2))
            .accessibilityHidden(true)
    }

    // MARK: Waiting on

    private var waiting: some View {
        VStack(alignment: .leading, spacing: 0) {
            Divider().padding(.leading, Tokens.space[3])
            Text(verbatim: card.waitingTitle)
                .font(TypeStyle.footnote.weight(.semibold))
                .foregroundStyle(Tokens.color(.label2))
                .padding(.horizontal, Tokens.space[3])
                .padding(.top, Tokens.space[2])
                .padding(.bottom, Tokens.space[0])
                .accessibilityAddTraits(.isHeader)
            if card.waitingOn.isEmpty {
                Text(verbatim: card.waitingEmptyText)
                    .font(TypeStyle.footnote)
                    .foregroundStyle(Tokens.color(.label2))
                    .padding(.horizontal, Tokens.space[3])
                    .padding(.bottom, Tokens.space[2])
            }
            ForEach(Array(card.waitingOn.enumerated()), id: \.element.id) { index, row in
                if index > 0 { Divider().padding(.leading, Tokens.space[3]) }
                if let link = row.link {
                    Button { actions.open(link) } label: { waitingLine(row, linked: true) }
                        .buttonStyle(PmRowStyle())
                        .accessibilityElement(children: .combine)
                } else {
                    waitingLine(row, linked: false).accessibilityElement(children: .combine)
                }
            }
        }
    }

    private func waitingLine(_ row: PmCardWaitingRow, linked: Bool) -> some View {
        HStack(spacing: Tokens.space[1]) {
            if typeSize.isAccessibilitySize {
                VStack(alignment: .leading, spacing: Tokens.space[0]) {
                    Text(verbatim: row.title).font(TypeStyle.body)
                    Text(verbatim: row.value).font(TypeStyle.body.monospacedDigit()).foregroundStyle(Tokens.color(.label2))
                }
                Spacer(minLength: Tokens.space[1])
            } else {
                Text(verbatim: row.title).font(TypeStyle.body)
                Spacer(minLength: Tokens.space[1])
                Text(verbatim: row.value)
                    .font(TypeStyle.subheadline.monospacedDigit())
                    .foregroundStyle(Tokens.color(.label2))
                    .lineLimit(1)
            }
            if linked { chevron }
        }
        .padding(.horizontal, Tokens.space[3])
        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
        .contentShape(Rectangle())
    }
}

/// A row that opens something: a fill while pressed.
private struct PmRowStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(configuration.isPressed ? Tokens.color(.fill2) : Color.clear)
    }
}

/// The yellow banner of an open stuck episode, inside the card: title, the reason's line, PM messages and
/// Pause project… (red outline). The buttons share a row, and stack one per row at accessibility sizes.
private struct PmStuckBanner: View {
    let stuck: PmCardStuck
    let actions: PmCardActions
    @Environment(\.dynamicTypeSize) private var typeSize

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
                Image(systemName: "exclamationmark.triangle")
                    .foregroundStyle(Tokens.color(.yellow))
                    .accessibilityHidden(true)
                Text(verbatim: stuck.title).font(TypeStyle.headline)
            }
            Text(verbatim: stuck.line)
                .font(TypeStyle.body)
                .fixedSize(horizontal: false, vertical: true)
            VStack(spacing: Tokens.space[1]) {
                Button { actions.open(.conversation) } label: {
                    Label { Text("PM messages") } icon: { Image(systemName: "bubble.left") }
                }
                .buttonStyle(.capstanQuiet)
                Button { actions.pause(stuck) } label: {
                    Label { Text("Pause project…") } icon: { Image(systemName: "pause") }
                }
                .buttonStyle(.capstanDestructive)
                .disabled(!actions.canPause)
            }
        }
        .foregroundStyle(Tokens.color(.label))
        .padding(Tokens.space[3])
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Tokens.color(.yellow).opacity(0.14))
        .accessibilityElement(children: .contain)
    }
}

/// The skeleton shown while the project's first snapshot is read: the card's shape and "Reading <host>…".
struct PmCardSkeleton: View {
    let hostName: String

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            Text("Project manager")
                .font(TypeStyle.footnote.weight(.semibold))
                .foregroundStyle(Tokens.color(.label2))
                .padding(.horizontal, Tokens.space[2])
                .accessibilityAddTraits(.isHeader)
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                Text(verbatim: "Reading " + hostName + "…").font(TypeStyle.subheadline).foregroundStyle(Tokens.color(.label2))
                bar(width: 0.5)
                bar(width: 0.9)
                bar(width: 0.7)
            }
            .padding(Tokens.space[3])
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Tokens.color(.cell), in: RoundedRectangle(cornerRadius: Tokens.radius.section))
            .accessibilityElement(children: .combine)
        }
    }

    private func bar(width: CGFloat) -> some View {
        RoundedRectangle(cornerRadius: Tokens.radius.control)
            .fill(Tokens.color(.fill2))
            .frame(height: 12)
            .containerRelativeFrame(.horizontal) { length, _ in length * width * 0.8 }
            .accessibilityHidden(true)
    }
}

#if DEBUG
#Preview("PmCardSkeleton") {
    PmCardSkeleton(hostName: "build-host").padding()
}
#endif
