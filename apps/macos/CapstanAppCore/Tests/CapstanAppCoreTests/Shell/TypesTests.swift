import CapstanKit
import XCTest

@testable import CapstanAppCore

final class TypesTests: XCTestCase {
    private let host = ShellFixtures.hostA

    private func hosts() throws -> [HostModel] {
        [ShellFixtures.host(host, projects: [try ShellFixtures.project("p1")])]
    }

    func test_SidebarSelection_codableRoundTrip() throws {
        let values: [SidebarSelection] = [
            .inbox, .hosts, .project(hostId: host, projectId: "p1"), .agents(hostId: host, projectId: "p1"),
            .plans(hostId: host, projectId: "p1"),
        ]
        for value in values {
            let data = try JSONEncoder().encode(value)
            XCTAssertEqual(try JSONDecoder().decode(SidebarSelection.self, from: data), value)
        }
    }

    func test_AppSettings_codableRoundTrip() throws {
        var settings = AppSettings()
        XCTAssertEqual(settings.clientLabel, "Mac")
        XCTAssertEqual(settings.notifyKinds, [.op, .prompt, .design, .message, .integration])
        XCTAssertFalse(settings.allowDuringFocus)
        XCTAssertTrue(settings.heartbeatPulse)
        settings.clientLabel = "Work Mac"
        settings.notifyKinds = [.op, .other("later")]
        settings.allowDuringFocus = true
        let data = try JSONEncoder().encode(settings)
        XCTAssertEqual(try JSONDecoder().decode(AppSettings.self, from: data), settings)
    }

    func test_SidebarSelection_resolvedProjectMissing() throws {
        let value = SidebarSelection.project(hostId: host, projectId: "gone")
        XCTAssertEqual(value.resolved(against: try hosts()), .inbox)
        let otherHost = SidebarSelection.project(hostId: ShellFixtures.hostB, projectId: "p1")
        XCTAssertEqual(otherHost.resolved(against: try hosts()), .inbox)
    }

    func test_SidebarSelection_resolvedProjectPresent() throws {
        let value = SidebarSelection.project(hostId: host, projectId: "p1")
        XCTAssertEqual(value.resolved(against: try hosts()), value)
    }

    func test_SidebarSelection_resolvedAgentsMissing() throws {
        let value = SidebarSelection.agents(hostId: host, projectId: "gone")
        XCTAssertEqual(value.resolved(against: try hosts()), .inbox)
    }

    func test_SidebarSelection_resolvedAgentsPresent() throws {
        let value = SidebarSelection.agents(hostId: host, projectId: "p1")
        XCTAssertEqual(value.resolved(against: try hosts()), value)
    }

    func test_SidebarSelection_resolvedPlansMissing() throws {
        let value = SidebarSelection.plans(hostId: host, projectId: "gone")
        XCTAssertEqual(value.resolved(against: try hosts()), .inbox)
    }

    func test_SidebarSelection_resolvedPlansPresent() throws {
        let value = SidebarSelection.plans(hostId: host, projectId: "p1")
        XCTAssertEqual(value.resolved(against: try hosts()), value)
    }

    func test_SidebarSelection_resolvedInboxAndHostsKept() {
        XCTAssertEqual(SidebarSelection.inbox.resolved(against: []), .inbox)
        XCTAssertEqual(SidebarSelection.hosts.resolved(against: []), .hosts)
    }

    func test_AppSheet_idNewAndEditing() {
        XCTAssertEqual(AppSheet.addHost(editing: nil).id, "addHost-new")
        let config = HostConfig(displayName: "Box", destination: "box")
        XCTAssertEqual(AppSheet.addHost(editing: config).id, "addHost-" + config.id.uuidString)
    }
}
