import CapstanAppCore
import CapstanKit
import SwiftUI

/// The connection state of a host as one word in a capsule; "Reconnecting" carries a countdown.
struct StatusCapsule: View {
    let model: AppModel
    let hostId: UUID?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var flash = false

    var body: some View {
        let state = hostId.flatMap { model.connection[$0] }
        let loading = anyLoading(hostId.flatMap { model.hostModels[$0] })
        let spec = CapsuleChoice.make(
            state: state, anyLoading: loading, heartbeatPulse: model.settings.heartbeatPulse,
            reduceMotion: reduceMotion)
        HStack(spacing: Tokens.space[1]) {
            Image(systemName: HostLabel.symbol(state))
                .imageScale(.small)
                .scaleEffect(flash ? 1.25 : 1)
            Text(verbatim: spec.label).font(.system(size: 12, weight: .medium))
            if case .reconnecting(_, let retryAt, _)? = state {
                Countdown(until: retryAt).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
        }
        .foregroundStyle(spec.tone.color)
        .padding(.horizontal, Tokens.space[2])
        .padding(.vertical, Tokens.space[0])
        .background(spec.tone.color.opacity(0.14), in: Capsule())
        .accessibilityElement(children: .combine)
        .onChange(of: lastLine(state)) { _, _ in beat(spec.pulse) }
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
            StatusCapsule(
                model: AppModel(preview: HostsPreviewFixtures.seed(connection: entry.state)),
                hostId: PreviewData.alphaId)
        }
        StatusCapsule(model: AppModel(preview: PreviewSeed()), hostId: nil)
    }
    .padding()
}
