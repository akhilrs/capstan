import CapstanAppCore
import CapstanKit
import SwiftUI

/// The footer capsule at the bottom left of the content column: `build-host live · heartbeat 3 s ago`, or the worst
/// state of several hosts, or `N hosts live`. It floats on glass; the dot carries the tone and the words carry the state.
struct StatusCapsule: View {
    let model: AppModel
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var flash = false

    var body: some View {
        // Only the age in the words depends on the clock; the tone, the dot and the name change with the model.
        let status = FooterStatus.make(hosts, now: .now)
        let pulse = CapsuleChoice.make(
            state: status.state, anyLoading: status.loading, heartbeatPulse: model.settings.heartbeatPulse,
            reduceMotion: reduceMotion
        ).pulse
        HStack(spacing: 0) {
            HStack(spacing: Tokens.space[1]) {
                if case .connecting? = status.state {
                    ProgressView().controlSize(.mini)
                } else {
                    Circle()
                        .fill(status.ring ? Color.clear : status.tone.color)
                        .overlay(Circle().strokeBorder(status.tone.color, lineWidth: status.ring ? 1.5 : 0))
                        .frame(width: 7, height: 7)
                        .scaleEffect(flash ? 1.5 : 1)
                }
                if let name = status.name {
                    Text(verbatim: name)
                        .font(.system(size: 12, weight: .semibold))
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                detail(status)
                    .font(.system(size: 12))
                    .foregroundStyle(Tokens.color(.label2))
                    .layoutPriority(1)
            }
            .padding(.horizontal, Tokens.space[2])
            .padding(.vertical, Tokens.space[0] + 2)
            .glassEffect(.regular, in: Capsule())
            .accessibilityElement(children: .combine)
            Spacer(minLength: 0)
        }
        .onChange(of: lastLine(status.state)) { _, _ in beat(pulse) }
    }

    /// The words after the name. Those that carry an age or a countdown redraw alone once a second.
    @ViewBuilder
    private func detail(_ status: FooterStatus) -> some View {
        if status.detail.contains(where: { $0.isASCII && $0.isNumber }) && status.name != nil {
            TickingText { now in FooterStatus.make(hosts, now: now).detail }
        } else {
            Text(verbatim: status.detail).lineLimit(1).fixedSize()
        }
    }

    private var hosts: [FooterStatus.Host] {
        model.hosts.map { config in
            FooterStatus.Host(
                name: HostLabel.hostName(config), state: model.connection[config.id],
                loading: anyLoading(model.hostModels[config.id]))
        }
    }

    private func lastLine(_ state: ConnectionState?) -> Date? {
        if case .live(_, let at)? = state { return at }
        return nil
    }

    private func beat(_ enabled: Bool) {
        guard enabled else { return }
        withAnimation(Motion.animation(Motion.pulse, reduce: reduceMotion)) { flash = true }
        Task {
            try? await Task.sleep(for: .milliseconds(Int(Motion.pulse * 1000)))
            withAnimation(Motion.animation(Motion.pulse, reduce: reduceMotion)) { flash = false }
        }
    }
}

#Preview("Status capsules") {
    VStack(alignment: .leading, spacing: Tokens.space[1]) {
        ForEach(Array(HostsPreviewFixtures.connections.enumerated()), id: \.offset) { _, entry in
            StatusCapsule(model: AppModel(preview: HostsPreviewFixtures.seed(connection: entry.state)))
        }
        StatusCapsule(model: AppModel(preview: PreviewSeed()))
    }
    .padding()
}
