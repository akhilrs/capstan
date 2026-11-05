import CapstanAppCore
import CapstanKit
import SwiftUI

/// What the PM card's controls do. The defaults do nothing, for previews.
struct PmCardActions {
    var open: (PmCardLink) -> Void = { _ in }
    var pause: (PmCardStuck) -> Void = { _ in }
    var refresh: () -> Void = {}
    /// False when the project cannot be paused from here (already paused, no decide, a call in flight).
    var canPause = true
}

/// The Project manager block at the top of a project's overview (plan-152, design-pm, Mac card): the header with the
/// PM's activity, the stuck banner when an episode is open, then the messages on the left and Waiting on on the right
/// (3:2), one column in a narrow window. Not glass: it holds host text.
struct PmCardView: View {
    let card: PmCardModel
    let actions: PmCardActions

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.space[2]) {
            Label { Text("Project manager") } icon: { Image(systemName: "person.crop.circle.badge.checkmark") }
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(Tokens.color(.label2))
                .labelStyle(.titleAndIcon)
                .accessibilityAddTraits(.isHeader)
            if let noPM = card.noPMText {
                Text(verbatim: noPM)
                    .font(.system(size: 13))
                    .foregroundStyle(Tokens.color(.label2))
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                header
                if let stuck = card.stuck {
                    StuckBanner(stuck: stuck, actions: actions)
                }
                ViewThatFits(in: .horizontal) {
                    HStack(alignment: .top, spacing: Tokens.space[3]) {
                        messages.frame(minWidth: 300, maxWidth: .infinity, alignment: .topLeading).layoutPriority(3)
                        waiting.frame(minWidth: 200, maxWidth: .infinity, alignment: .topLeading).layoutPriority(2)
                    }
                    VStack(alignment: .leading, spacing: Tokens.space[2]) {
                        messages
                        waiting
                    }
                }
            }
        }
        .padding(Tokens.space[2])
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Tokens.color(.fill), in: RoundedRectangle(cornerRadius: Tokens.radius.block))
        .overlay(RoundedRectangle(cornerRadius: Tokens.radius.block).strokeBorder(Tokens.color(.sep)))
    }

    // MARK: Header

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: Tokens.space[1]) {
            Text(verbatim: card.agentId ?? "")
                .font(.system(size: 13, weight: .semibold, design: .monospaced))
            StatePill(
                word: card.activityWord, symbol: card.activitySymbol,
                tone: card.health.map(AgentTone.tone) ?? .grey)
            if let since = card.sinceText {
                Text(verbatim: since)
                    .font(.system(size: 12).monospacedDigit())
                    .foregroundStyle(Tokens.color(.label2))
                    .lineLimit(1)
            }
            Spacer(minLength: Tokens.space[1])
            freshness
        }
    }

    @ViewBuilder
    private var freshness: some View {
        switch card.freshness {
        case .current:
            EmptyView()
        case .lastKnown(let time):
            StatePill(word: "Last known · " + time, symbol: "clock", tone: .grey)
        case .stale(let time):
            HStack(spacing: Tokens.space[1]) {
                StatePill(word: "Stale · as of " + time, symbol: "exclamationmark.triangle", tone: .yellow)
                Button("Refresh", action: actions.refresh)
                    .buttonStyle(.capstanQuiet)
            }
        }
    }

    // MARK: Columns

    private var messages: some View {
        VStack(alignment: .leading, spacing: Tokens.space[1]) {
            CardRow(action: { actions.open(.conversation) }) {
                MessagePreview(
                    title: "From the PM", message: card.fromPM, emptyText: card.fromPMEmptyText, delivery: nil)
            }
            CardRow(action: { actions.open(.conversation) }) {
                MessagePreview(
                    title: "You sent", message: card.youSent, emptyText: card.youSentEmptyText,
                    delivery: card.delivery)
            }
            Button { actions.open(.conversation) } label: { Text("Open conversation ›") }
                .buttonStyle(.capstanQuiet)
        }
    }

    private var waiting: some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            Text(verbatim: card.waitingTitle)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(Tokens.color(.label2))
                .accessibilityAddTraits(.isHeader)
            if card.waitingOn.isEmpty {
                Text(verbatim: card.waitingEmptyText)
                    .font(.system(size: 13))
                    .foregroundStyle(Tokens.color(.label2))
            }
            ForEach(card.waitingOn) { row in
                if let link = row.link {
                    CardRow(action: { actions.open(link) }) { waitingLine(row) }
                } else {
                    waitingLine(row).padding(.horizontal, Tokens.space[1]).frame(minHeight: 28)
                }
            }
        }
    }

    private func waitingLine(_ row: PmCardWaitingRow) -> some View {
        HStack(spacing: Tokens.space[1]) {
            Text(verbatim: row.title).font(.system(size: 13))
            Spacer(minLength: Tokens.space[1])
            Text(verbatim: row.value)
                .font(.system(size: 12).monospacedDigit())
                .foregroundStyle(Tokens.color(.label2))
                .lineLimit(1)
        }
        .accessibilityElement(children: .combine)
    }
}

/// A row that opens something: a hover fill and a chevron, 28 pt or taller.
private struct CardRow<Inner: View>: View {
    let action: () -> Void
    @ViewBuilder let inner: () -> Inner
    @State private var hovering = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    init(action: @escaping () -> Void, @ViewBuilder inner: @escaping () -> Inner) {
        self.action = action
        self.inner = inner
    }

    var body: some View {
        Button(action: action) {
            HStack(spacing: Tokens.space[1]) {
                inner()
                Image(systemName: "chevron.right")
                    .imageScale(.small)
                    .foregroundStyle(Tokens.color(.label2))
                    .accessibilityHidden(true)
            }
            .padding(.horizontal, Tokens.space[1])
            .padding(.vertical, Tokens.space[0])
            .frame(minHeight: 28)
            .contentShape(Rectangle())
            .background(
                hovering ? Tokens.color(.fill2) : Color.clear, in: RoundedRectangle(cornerRadius: Tokens.radius.control))
        }
        .buttonStyle(.plain)
        .onHover { inside in
            withAnimation(Motion.animation(Motion.hover, reduce: reduceMotion)) { hovering = inside }
        }
    }
}

/// Two lines of untrusted text with its time, or the empty line; the delivery line under the person's own message.
private struct MessagePreview: View {
    let title: String
    let message: PmCardMessage?
    let emptyText: String
    let delivery: PmCardDelivery?

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: Tokens.space[1]) {
                Text(verbatim: title).font(.system(size: 12, weight: .semibold)).foregroundStyle(Tokens.color(.label2))
                Spacer(minLength: 0)
                if let time = message?.time, !time.isEmpty {
                    Text(verbatim: time).font(.system(size: 12).monospacedDigit()).foregroundStyle(Tokens.color(.label2))
                }
            }
            if let message {
                if let text = message.text {
                    Text(verbatim: text)
                        .font(.system(size: 12, design: .monospaced))
                        .lineLimit(2)
                        .truncationMode(.tail)
                        .frame(maxWidth: .infinity, alignment: .leading)
                } else {
                    Text("Text not shown: the host could not vouch for it")
                        .font(.system(size: 12))
                        .foregroundStyle(Tokens.color(.label2))
                }
            } else {
                Text(verbatim: emptyText).font(.system(size: 13)).foregroundStyle(Tokens.color(.label2))
            }
            if let delivery {
                Label { Text(verbatim: delivery.text) } icon: { Image(systemName: delivery.symbol) }
                    .font(.system(size: 12))
                    .foregroundStyle(delivery.failed ? Tokens.color(.red) : Tokens.color(.label2))
            }
        }
        .accessibilityElement(children: .combine)
    }
}

/// The yellow banner of an open stuck episode, inside the card: a title, the reason's line, PM messages and
/// Pause project….
private struct StuckBanner: View {
    let stuck: PmCardStuck
    let actions: PmCardActions

    var body: some View {
        HStack(alignment: .top, spacing: Tokens.space[1]) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(Tokens.color(.yellow))
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: Tokens.space[1]) {
                Text(verbatim: stuck.title).font(.system(size: 13, weight: .semibold))
                Text(verbatim: stuck.line)
                    .font(.system(size: 13))
                    .fixedSize(horizontal: false, vertical: true)
                HStack(spacing: Tokens.space[1]) {
                    Button("PM messages") { actions.open(.conversation) }
                        .buttonStyle(.capstanQuiet)
                    Button { actions.pause(stuck) } label: {
                        Text("Pause project…").foregroundStyle(Tokens.color(.red))
                    }
                    .buttonStyle(.capstanQuiet)
                    .disabled(!actions.canPause)
                }
            }
            Spacer(minLength: 0)
        }
        .foregroundStyle(Tokens.color(.label))
        .padding(Tokens.space[2])
        .background(Tokens.color(.yellow).opacity(0.14), in: RoundedRectangle(cornerRadius: Tokens.radius.control))
        .accessibilityElement(children: .contain)
    }
}

#if DEBUG
private enum PmCardSamples {
    static let now: Date = ISO8601DateFormatter().date(from: "2026-10-29T12:31:00Z") ?? Date()

    static func card(stuck: String? = nil, pm: Bool = true, empty: Bool = false) -> PmCardModel? {
        var project = OverviewSamples.project()
        project.run = StoredRun(
            requestId: "run", state: "active", paused: false, contentHash: "h",
            pm: pm
                ? PmCard(
                    agentId: "pm-1",
                    waitingOn: empty ? [] : [
                        PmCard.Waiting(kind: "report", count: 3, since: "2026-10-29T10:21:00Z"),
                        PmCard.Waiting(kind: "review", count: 1, since: "2026-10-29T12:19:00Z"),
                        PmCard.Waiting(kind: "user", count: 2, since: "2026-10-29T12:01:00Z"),
                    ],
                    fromUser: empty ? nil : PmCard.FromUser(
                        messageId: "msg-12", createdAt: "2026-10-29T12:03:00Z", state: "sent",
                        stateSince: "2026-10-29T12:05:00Z", delivery: "read",
                        body: UntrustedText(raw: "Which database should the tests use?", verified: true, truncated: false)),
                    stuck: stuck.map {
                        PmCard.Stuck(episodeId: "stuck-1", reason: $0, since: "2026-10-29T12:14:00Z", alert: true)
                    })
                : nil)
        return PmCardModel.make(
            project: project, projectName: "capstan",
            freshness: ProjectFreshness(currentAsOf: now), now: now, calendar: OverviewSamples.calendar)
    }
}

#Preview("PmCard") {
    if let card = PmCardSamples.card() { PmCardView(card: card, actions: PmCardActions()).padding().frame(width: 720) }
}

#Preview("PmCardStuck") {
    if let card = PmCardSamples.card(stuck: "unread_mail") {
        PmCardView(card: card, actions: PmCardActions()).padding().frame(width: 720)
    }
}

#Preview("PmCardCompact") {
    if let card = PmCardSamples.card(stuck: "prompt") {
        PmCardView(card: card, actions: PmCardActions()).padding().frame(width: 440)
    }
}

#Preview("PmCardNothingYet") {
    if let card = PmCardSamples.card(empty: true) { PmCardView(card: card, actions: PmCardActions()).padding().frame(width: 720) }
}

#Preview("PmCardNoPM") {
    if let card = PmCardSamples.card(pm: false) { PmCardView(card: card, actions: PmCardActions()).padding().frame(width: 720) }
}
#endif
