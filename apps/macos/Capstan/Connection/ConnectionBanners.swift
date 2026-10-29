import CapstanAppCore
import CapstanKit
import SwiftUI

/// The banners a host's connection needs: what is wrong, the host's words about it and the action that helps.
struct ConnectionBanners: View {
    let model: AppModel
    let hostId: UUID

    var body: some View {
        if let config = model.hosts.first(where: { $0.id == hostId }) {
            TimelineView(.periodic(from: .now, by: 1)) { context in
                let specs = BannerChoice.make(
                    state: model.connection[hostId], anyLoading: anyLoading(model.hostModels[hostId]),
                    decide: model.capabilities[hostId]?.decide, hostName: config.displayName,
                    destination: config.destination, now: context.date)
                VStack(alignment: .leading, spacing: Tokens.space[1]) {
                    ForEach(Array(specs.enumerated()), id: \.offset) { _, spec in
                        banner(spec)
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func banner(_ spec: BannerSpec) -> some View {
        VStack(alignment: .leading, spacing: Tokens.space[0]) {
            Banner(
                tone: spec.tone, text: spec.text, actionTitle: actionTitle(spec.action),
                action: spec.action.map { a in { () -> Void in perform(a) } })
            if let detail = spec.detail {
                UntrustedTextBlock(text: detail, label: "Output from the host")
            }
            if let retryAt = spec.retryAt {
                Countdown(until: retryAt).font(.system(size: 12)).foregroundStyle(Tokens.color(.label2))
            }
        }
    }

    private func actionTitle(_ action: BannerAction?) -> String? {
        switch action {
        case .retry?: return "Retry"
        case .editHost?: return "Edit host"
        case nil: return nil
        }
    }

    private func perform(_ action: BannerAction) {
        let config = model.hosts.first(where: { $0.id == hostId })
        switch BannerChoice.resolveAction(action, configExists: config != nil, sheetPresented: model.presented != nil) {
        case .restart: model.restart(hostId: hostId)
        case .openEditor: model.presented = .addHost(editing: config)
        case .none: break
        }
    }
}

#Preview("Banner lists") {
    ScrollView {
        VStack(alignment: .leading, spacing: Tokens.space[3]) {
            ForEach(Array(HostsPreviewFixtures.connections.enumerated()), id: \.offset) { _, entry in
                ConnectionBanners(
                    model: AppModel(preview: HostsPreviewFixtures.seed(connection: entry.state)),
                    hostId: PreviewData.alphaId)
            }
            ConnectionBanners(
                model: AppModel(preview: HostsPreviewFixtures.seed(connection: .connecting, decide: false)),
                hostId: PreviewData.alphaId)
        }
        .padding()
    }
    .frame(width: 520, height: 700)
}
