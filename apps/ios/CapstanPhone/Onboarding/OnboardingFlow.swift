import CapstanKit
import CapstanPhoneCore
import SwiftUI

extension AppModel {
    /// The first run, or a restored phone: no host yet, or no key (an enclave key does not survive a restore).
    var needsOnboarding: Bool {
        if case .present = key { return hosts.isEmpty }
        return true
    }
}

extension View {
    /// The first-run flow over the tabs, and the Add a host sheet. `enabled` is false while the app is locked, so
    /// nothing shows over the Face ID cover.
    func onboarding(_ model: AppModel, enabled: Bool) -> some View {
        modifier(OnboardingPresenter(model: model, enabled: enabled))
    }
}

private struct OnboardingPresenter: ViewModifier {
    @Bindable var model: AppModel
    let enabled: Bool

    func body(content: Content) -> some View {
        content
            .fullScreenCover(isPresented: Binding(get: { enabled && model.needsOnboarding }, set: { _ in })) {
                OnboardingFlow(model: model)
            }
            .sheet(isPresented: Binding(
                get: { enabled && !model.needsOnboarding && model.navigation.sheet == .addHost },
                set: { if !$0, model.navigation.sheet == .addHost { model.navigation.sheet = nil } }
            )) {
                NavigationStack {
                    AddHostForm(model: model, onDone: { model.navigation.sheet = nil })
                }
            }
    }
}

/// Welcome, the key, then adding the first host. A phone that has hosts but no key starts on the key.
struct OnboardingFlow: View {
    @Bindable var model: AppModel
    @State private var path: [Step] = []

    enum Step: Hashable {
        case key, addHost
    }

    var body: some View {
        NavigationStack(path: $path) {
            root
                .navigationDestination(for: Step.self) { step in
                    switch step {
                    case .key: KeyScreen(model: model, next: { path.append(.addHost) })
                    case .addHost: AddHostForm(model: model, onDone: {})
                    }
                }
        }
        .tint(Tokens.color(.accent))
    }

    @ViewBuilder private var root: some View {
        if model.hosts.isEmpty {
            WelcomeScreen(next: {
                if case .none = model.key { model.makeKey(.secureEnclaveP256) }
                path.append(.key)
            })
        } else {
            KeyScreen(model: model, next: {})
        }
    }
}

/// The bottom action bar of the design: one primary action, on the floating layer.
struct ActionBar<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        VStack(spacing: Tokens.space[2]) { content }
            .padding(.horizontal, Tokens.margin)
            .padding(.top, Tokens.space[2])
            .padding(.bottom, Tokens.space[1])
    }
}
