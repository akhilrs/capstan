import CapstanKit
import Foundation

/// One line of host text in a list, with the trusted caption above it (ids, a time) and the words the design shows when
/// the text failed its integrity check or was cut.
public struct DetailEntry: Sendable, Equatable {
    public let caption: String?
    public let text: UntrustedText
    /// A count set at the right of the line (the files of an area); nil for other lines.
    public let count: String?
    /// The kind of text, as the words "<Kind> not shown" and "The full <kind> is on the host" name it.
    let kind: String
    let readInTerminal: Bool

    init(caption: String?, text: UntrustedText, kind: String, count: String? = nil, readInTerminal: Bool = false) {
        self.caption = caption
        self.text = text
        self.count = count
        self.kind = kind
        self.readInTerminal = readInTerminal
    }

    /// The text to show; nil when the host could not vouch for it. A cut text ends with "…".
    public var shown: String? {
        guard text.verified else { return nil }
        return text.truncated ? text.displayPlain + "…" : text.displayPlain
    }

    /// The line that stands in for text that failed its integrity check; nil when the text is shown.
    public var notShown: String? {
        guard !text.verified else { return nil }
        let head = String(kind.prefix(1)).uppercased()
        let tail = String(kind.dropFirst())
        let line = head + tail + " not shown: it failed its integrity check."
        return readInTerminal ? line + " Read it in the terminal." : line
    }

    /// The line that follows a text the host cut; nil when the text is whole or not shown.
    public var cutNote: String? {
        text.verified && text.truncated ? "Cut by the host. The full " + kind + " is on the host." : nil
    }
}

/// The result of a check: a word with a tone, and the lines that qualify it.
public struct DetailResult: Sendable, Equatable {
    public let label: String
    public let word: String
    public let tone: Tone
    public let detail: String?
    public let notes: [String]
}

public enum DetailBlock: Sendable, Equatable {
    /// A plain sentence of the app.
    case note(String)
    case banner(tone: Tone, title: String, text: String)
    /// The files, insertions and deletions of the diff.
    case stat(files: String, added: String, removed: String)
    /// A labelled list of host text; `more` is the "and N more" line of what the host left out.
    case entries(label: String, entries: [DetailEntry], more: String?)
    case results([DetailResult])
}

public struct DetailSection: Sendable, Equatable {
    public let title: String
    /// The line that stays visible when the section is folded; nil when the section has nothing to sum up.
    public let summary: String?
    public let blocks: [DetailBlock]
}

public struct DetailMeta: Sendable, Equatable {
    public let label: String
    public let value: UntrustedText
}

public enum StepAction: Sendable, Equatable {
    /// The app merges the branch (the Merge button of plan-130).
    case merge
}

public struct DetailStep: Sendable, Equatable {
    public let title: String?
    public let note: String?
    /// A command the app builds for the person to run; the branch in it is quoted.
    public let command: String?
    public let action: StepAction?
    /// A step the host names; shown as untrusted text.
    public let hostText: DetailEntry?
}

/// The inspector body of an integration item: the state in words, the purpose, impact and results of the integration,
/// and the next steps. The words never include a bare "merged": the feed's `merged` means the reports merged into the
/// integration branch, not into the project's main branch.
public struct IntegrationDetailRows: Sendable, Equatable {
    public let stateWord: String
    public let stateTone: Tone
    public let title: String
    public let lead: String?
    public let meta: [DetailMeta]
    public let banners: [DetailBlock]
    public let sections: [DetailSection]
    public let steps: [DetailStep]

    public static let builtWord = "Built · not in main"
    public static let conflictedWord = "Conflicted · nothing to merge"

    /// `mergeAvailable` is true when the inspector offers the app's own Merge for this item; without it, and always
    /// when the app has no Merge, the next steps are the terminal commands.
    public static func make(
        _ item: StoredWaiting, hostName: String, now: Date, mergeAvailable: Bool = false,
        calendar: Calendar = Calendar(identifier: .gregorian)
    ) -> IntegrationDetailRows {
        let words = item.summary.displayPlain.split(separator: " ", maxSplits: 1).map(String.init)
        let state = item.integrationState ?? words.first ?? ""
        let branch = item.branch ?? UntrustedText(
            raw: words.count > 1 ? words[1] : "", verified: item.summary.verified, truncated: false)
        let details = item.integrationDetails
        var reportCount: Int?
        if let details, details.recorded { reportCount = details.reports.count + Int(details.reportsOmitted) }
        let headLine = headTitle(state: state, branch: branch)
        var banners: [DetailBlock] = []
        var sections: [DetailSection] = []
        if state == "conflicted" {
            banners.append(
                .banner(
                    tone: .yellow, title: "The reports conflict with each other",
                    text: "Building the integration branch stopped at a conflict. Nothing reached main and nothing is "
                        + "ready to merge."))
        }
        if let details {
            if details.recorded {
                sections = recordedSections(details, item: item, now: now, calendar: calendar)
            } else if state == "conflicted" {
                sections = [
                    DetailSection(
                        title: "What it is for · Impact · Results", summary: nil,
                        blocks: [
                            .note(
                                "Not recorded. Capstan records these only when an integration merges cleanly; a "
                                    + "conflicted one never has them. This is expected.")
                        ])
                ]
            } else {
                banners.append(
                    .banner(
                        tone: .grey, title: "Details not recorded",
                        text: "This integration was built before Capstan kept its purpose, impact and results. Run "
                            + "cstan integrate show on the host to see its reports."))
            }
        } else {
            banners.append(
                .banner(
                    tone: .grey, title: "No details from this host",
                    text: "This host's Capstan does not send integration details. Update Capstan on " + hostName
                        + " to see purpose, impact and results. The next steps below still apply."))
        }
        return IntegrationDetailRows(
            stateWord: stateWord(state), stateTone: stateTone(state), title: headLine,
            lead: lead(state: state, reportCount: reportCount), meta: metaRows(branch: branch, headSha: item.headSha),
            banners: banners, sections: sections,
            steps: steps(
                state: state, branch: branch, hostName: hostName, details: details, mergeAvailable: mergeAvailable))
    }

    // MARK: State

    public static func stateWord(_ state: String) -> String {
        switch state {
        case "merged": return builtWord
        case "conflicted": return conflictedWord
        default: return "Integration"
        }
    }

    static func stateTone(_ state: String) -> Tone {
        switch state {
        case "merged": return .blue
        case "conflicted": return .yellow
        default: return .grey
        }
    }

    private static func headTitle(state: String, branch: UntrustedText) -> String {
        let name = branch.verified && !branch.displayPlain.isEmpty ? branch.displayPlain : "The integration"
        switch state {
        case "merged": return name + " is built, not in main yet"
        case "conflicted": return name + " has a conflict"
        default: return name
        }
    }

    private static func lead(state: String, reportCount: Int?) -> String? {
        guard state == "merged" else { return nil }
        let reports = reportCount.map { $0 == 1 ? "Its 1 report" : "Its " + String($0) + " reports" } ?? "Its reports"
        return reports + " merged cleanly into the integration branch. The project's main branch does not have them "
            + "until the branch is merged."
    }

    private static func metaRows(branch: UntrustedText, headSha: String?) -> [DetailMeta] {
        var rows = [DetailMeta(label: "Branch", value: branch)]
        if let headSha {
            rows.append(
                DetailMeta(
                    label: "Head", value: UntrustedText(raw: String(headSha.prefix(12)), verified: true, truncated: false)))
        }
        return rows
    }

    // MARK: Sections

    private static func recordedSections(
        _ details: IntegrationDetails, item: StoredWaiting, now: Date, calendar: Calendar
    ) -> [DetailSection] {
        [
            purposeSection(details, now: now, calendar: calendar),
            impactSection(details, branch: item.branch),
            resultsSection(details, headSha: item.headSha, now: now, calendar: calendar),
        ]
    }

    static func purposeSection(_ details: IntegrationDetails, now: Date, calendar: Calendar) -> DetailSection {
        var blocks: [DetailBlock] = []
        if details.plans.isEmpty {
            blocks.append(.note("No plan: these reports were integrated outside a plan."))
        } else {
            let entries = details.plans.map {
                DetailEntry(caption: $0.planId, text: $0.title, kind: "title")
            }
            let label = entries.count == 1 && details.plansOmitted == 0
                ? "Plan · untrusted title" : "Plans · untrusted titles"
            blocks.append(.entries(label: label, entries: entries, more: more(details.plansOmitted, "plan")))
        }
        if details.signoffs.isEmpty {
            blocks.append(.note("Not signed off yet."))
        } else {
            let planIds = Set(details.plans.map { $0.planId })
            let entries = details.signoffs.map { signoff -> DetailEntry in
                var caption = signoff.planId + " · " + clock(signoff.createdAt, now: now, calendar: calendar)
                if !planIds.contains(signoff.planId) { caption += " · not in this integration's plans" }
                return DetailEntry(caption: caption, text: signoff.summary, kind: "text")
            }
            blocks.append(
                .entries(
                    label: "Architect sign-off · untrusted text", entries: entries,
                    more: more(details.signoffsOmitted, "sign-off")))
        }
        if details.reports.isEmpty {
            blocks.append(.note("No reports listed."))
        } else {
            let entries = details.reports.map {
                DetailEntry(
                    caption: $0.reportId + " · " + ($0.packageId ?? "no package"), text: $0.summary, kind: "summary",
                    readInTerminal: true)
            }
            blocks.append(
                .entries(
                    label: "Reports · untrusted summaries", entries: entries, more: more(details.reportsOmitted, "report")))
        }
        return DetailSection(title: "What it is for", summary: purposeSummary(details), blocks: blocks)
    }

    private static func purposeSummary(_ details: IntegrationDetails) -> String {
        let reports = Int(details.reports.count) + Int(details.reportsOmitted)
        let plan = details.plans.first?.planId ?? "no plan"
        let count = reports == 1 ? "1 report" : String(reports) + " reports"
        let signed = details.signoffs.isEmpty ? "not signed off" : "signed off"
        return plan + " · " + count + " · " + signed
    }

    static func impactSection(_ details: IntegrationDetails, branch: UntrustedText?) -> DetailSection {
        var blocks: [DetailBlock] = []
        if let error = details.impactError {
            blocks.append(impactErrorBanner(error, branch: branch))
        }
        guard let impact = details.impact else {
            if details.impactError == nil { blocks.append(.note("Impact not recorded.")) }
            let summary = details.impactError == nil ? "not recorded" : "not measured"
            return DetailSection(title: "Impact", summary: summary, blocks: blocks)
        }
        let files = impact.filesChanged == 1 ? "1 file changed" : grouped(impact.filesChanged) + " files changed"
        let prefix = impact.truncated ? "At least " : ""
        blocks.append(
            .stat(files: prefix + files, added: "+" + grouped(impact.insertions), removed: "−" + grouped(impact.deletions)))
        if impact.truncated {
            blocks.append(
                .note(
                    "The diff was too large to count in full; these are the counts up to where the host stopped."))
        }
        if !impact.areas.isEmpty {
            let entries = impact.areas.map {
                DetailEntry(caption: nil, text: $0.path, kind: "path", count: grouped($0.files))
            }
            blocks.append(
                .entries(label: "Areas · untrusted paths", entries: entries, more: more(impact.areasOmitted, "area")))
        }
        if impact.migrations.isEmpty && impact.migrationsOmitted == 0 {
            blocks.append(.note("Migrations: none."))
        } else {
            let total = impact.migrations.count + Int(impact.migrationsOmitted)
            let entries = impact.migrations.map { DetailEntry(caption: nil, text: $0.path, kind: "path") }
            blocks.append(
                .entries(
                    label: "Migrations · " + String(total) + " · untrusted paths", entries: entries,
                    more: more(impact.migrationsOmitted, "migration")))
        }
        let fileWords = impact.filesChanged == 1 ? "1 file" : grouped(impact.filesChanged) + " files"
        let counts = "+" + grouped(impact.insertions) + " −" + grouped(impact.deletions)
        let summary = prefix.lowercased() + fileWords + " · " + counts
        return DetailSection(title: "Impact", summary: summary, blocks: blocks)
    }

    private static func impactErrorBanner(_ error: String, branch: UntrustedText?) -> DetailBlock {
        switch error {
        case "timeout":
            return .banner(
                tone: .grey, title: "Impact not measured: the diff timed out",
                text: "The host stopped measuring the diff after its time limit. The merge itself is not affected.")
        case "diff_failed":
            var command = "git diff --stat"
            if let branch, let name = plainBranch(branch) { command = "git diff --stat HEAD..." + shellWord(name) }
            return .banner(
                tone: .grey, title: "Impact not measured: git could not diff",
                text: "git failed to compare the branch with main on the host. Run " + command
                    + " there to see it.")
        default:
            return .banner(
                tone: .grey, title: "Impact not measured", text: "The host could not work out the impact of the diff.")
        }
    }

    static func resultsSection(
        _ details: IntegrationDetails, headSha: String?, now: Date, calendar: Calendar
    ) -> DetailSection {
        let review = reviewResult(details.review)
        let verify = verifyResult(details.verify, headSha: headSha, now: now, calendar: calendar)
        let summary = reviewSummary(details.review) + " · " + verifySummary(details.verify, headSha: headSha)
        return DetailSection(
            title: "Results", summary: summary,
            blocks: [
                .results([review, verify]),
                .note("Finished results only. A review in progress and a queued or running verify are not shown."),
            ])
    }

    static func reviewResult(_ review: IntegrationReview?) -> DetailResult {
        guard let review else {
            return DetailResult(label: "Review", word: "Not reviewed yet", tone: .grey, detail: nil, notes: [])
        }
        let detail = review.reviewerAgentId + " · round " + String(review.round)
        return DetailResult(
            label: "Review", word: reviewWord(review.verdict), tone: reviewTone(review.verdict), detail: detail, notes: [])
    }

    private static func reviewWord(_ verdict: String) -> String {
        switch verdict {
        case "passed": return "Passed"
        case "findings": return "Findings"
        case "failed": return "Failed"
        case "cancelled": return "Cancelled"
        default: return "Unknown"
        }
    }

    private static func reviewTone(_ verdict: String) -> Tone {
        switch verdict {
        case "passed": return .green
        case "findings": return .yellow
        case "failed": return .red
        default: return .grey
        }
    }

    private static func reviewSummary(_ review: IntegrationReview?) -> String {
        guard let review else { return "not reviewed" }
        return "review " + reviewWord(review.verdict).lowercased()
    }

    static func verifyResult(
        _ verify: IntegrationVerify?, headSha: String?, now: Date, calendar: Calendar
    ) -> DetailResult {
        guard let verify else {
            return DetailResult(
                label: "Verify", word: "Never run", tone: .grey, detail: "no verify has finished for this branch", notes: [])
        }
        let latest = verify.latest
        var notes: [String] = []
        let word: String
        let tone: Tone
        let detail: String?
        if sameCommit(latest.commitSha, headSha) {
            word = verifyWord(latest.state)
            tone = verifyTone(latest.state)
            detail = "on this commit · " + ago(latest.finishedAt, now: now, calendar: calendar)
        } else {
            word = "Not run on this commit"
            tone = .grey
            detail = nil
            let sha = String(latest.commitSha.prefix(12))
            let when = ago(latest.finishedAt, now: now, calendar: calendar)
            notes.append("Latest: " + verifyWord(latest.state) + " on " + sha + " · " + when)
        }
        if let passed = verify.lastPassed {
            let sameRun = latest.state == "passed" && passed.commitSha == latest.commitSha
                && passed.finishedAt == latest.finishedAt
            if !sameRun {
                let sha = String(passed.commitSha.prefix(12))
                let when = ago(passed.finishedAt, now: now, calendar: calendar)
                notes.append("Last passed on " + sha + " · " + when)
            }
        }
        return DetailResult(label: "Verify", word: word, tone: tone, detail: detail, notes: notes)
    }

    private static func verifyWord(_ state: String) -> String {
        switch state {
        case "passed": return "Passed"
        case "failed": return "Failed"
        case "timeout": return "Timed out"
        case "error": return "Error"
        default: return "Unknown"
        }
    }

    private static func verifyTone(_ state: String) -> Tone {
        switch state {
        case "passed": return .green
        case "failed", "timeout", "error": return .red
        default: return .grey
        }
    }

    private static func verifySummary(_ verify: IntegrationVerify?, headSha: String?) -> String {
        guard let verify else { return "never verified" }
        if !sameCommit(verify.latest.commitSha, headSha) { return "verify not run on this commit" }
        return "verify " + verifyWord(verify.latest.state).lowercased()
    }

    private static func sameCommit(_ left: String, _ right: String?) -> Bool {
        guard let right else { return false }
        return left.hasPrefix(right) || right.hasPrefix(left)
    }

    // MARK: Next steps

    private static func steps(
        state: String, branch: UntrustedText, hostName: String, details: IntegrationDetails?, mergeAvailable: Bool
    ) -> [DetailStep] {
        if state == "conflicted" {
            return [
                DetailStep(
                    title: "The project manager has a developer resolve it",
                    note: "The fix arrives as a new report. Nothing to do here unless you want to tell the project "
                        + "manager something.", command: nil, action: nil, hostText: nil),
                DetailStep(
                    title: "A new integration replaces this one",
                    note: "When it is built, it shows up as its own item with its purpose, impact and results. This "
                        + "item then leaves the inbox.", command: nil, action: nil, hostText: nil),
            ]
        }
        guard state == "merged" else { return [] }
        var result: [DetailStep] = []
        if mergeAvailable {
            result.append(
                DetailStep(
                    title: "Merge it into main",
                    note: "Opens the confirmation sheet. After the merge Capstan confirms the integration itself and "
                        + "tells the project manager; no confirm step is left.", command: nil, action: .merge,
                    hostText: nil))
        } else {
            result.append(terminalMergeStep(branch: branch, hostName: hostName))
            result.append(
                DetailStep(
                    title: "The project manager confirms the merge",
                    note: "Tell the project manager it is in main. They run cstan integrate confirm, which removes "
                        + "the branch.", command: nil, action: nil, hostText: nil))
        }
        if let details, details.recorded, let impact = details.impact {
            for step in impact.nextSteps {
                let entry = DetailEntry(caption: nil, text: step.text, kind: "step")
                result.append(DetailStep(title: nil, note: nil, command: nil, action: nil, hostText: entry))
            }
        }
        return result
    }

    /// The terminal commands that merge the branch, shown as a step of their own.
    static func terminalMergeStep(branch: UntrustedText, hostName: String) -> DetailStep {
        let command = mergeCommand(branch)
        let note = command == nil
            ? "The branch name cannot be shown safely here. Read it on " + hostName + " and merge it from the terminal."
            : "In the project's checkout on " + hostName + ", with the branch to merge into checked out:"
        return DetailStep(title: "Merge it into main", note: note, command: command, action: nil, hostText: nil)
    }

    /// The command for a branch the host vouched for and that holds no control or look-alike character; nil otherwise,
    /// so the person reads the name before it goes near a shell. The name is quoted.
    static func mergeCommand(_ branch: UntrustedText) -> String? {
        guard let name = plainBranch(branch) else { return nil }
        return "git merge --no-ff " + shellWord(name)
    }

    /// The word as it is written in a command: bare when it holds only characters a shell leaves alone, else quoted.
    static func shellWord(_ name: String) -> String {
        let plain = name.unicodeScalars.allSatisfy { scalar in
            switch scalar.value {
            case 0x30...0x39, 0x41...0x5A, 0x61...0x7A: return true
            default: return "._/@+=,:-".unicodeScalars.contains(scalar)
            }
        }
        return plain ? name : ShellQuote.quote(name)
    }

    /// The branch name when it is vouched for and holds nothing a shell or a reader could mistake: no control or
    /// direction character, and no leading dash.
    private static func plainBranch(_ branch: UntrustedText) -> String? {
        guard branch.verified, !branch.raw.isEmpty, branch.displayPlain == branch.raw, !branch.raw.hasPrefix("-") else {
            return nil
        }
        return branch.raw
    }

    // MARK: Text

    private static func more(_ omitted: Int64, _ noun: String) -> String? {
        guard omitted > 0 else { return nil }
        let plural = omitted == 1 ? "" : "s"
        return "and " + grouped(omitted) + " more " + noun + plural
    }

    /// 1284 as "1,284".
    static func grouped(_ number: Int64) -> String {
        let digits = Array(String(number.magnitude))
        var out = ""
        for (index, digit) in digits.enumerated() {
            if index > 0 && (digits.count - index) % 3 == 0 { out += "," }
            out.append(digit)
        }
        return number < 0 ? "-" + out : out
    }

    /// "2 h ago" for a time earlier today; an earlier day reads as "yesterday 17:05" or "Oct 24".
    static func ago(_ since: String, now: Date, calendar: Calendar) -> String {
        guard let date = MenuList.date(since) else { return since }
        if calendar.isDate(date, inSameDayAs: now) {
            let age = AgeText.short(since: since, now: now)
            return age == "now" ? "just now" : age + " ago"
        }
        return clock(since, now: now, calendar: calendar)
    }

    /// "today 12:40", "yesterday 17:05" or "Oct 24".
    static func clock(_ since: String, now: Date, calendar: Calendar) -> String {
        guard let date = MenuList.date(since) else { return since }
        let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
        let parts = calendar.dateComponents([.month, .day, .hour, .minute], from: date)
        let hour = parts.hour ?? 0
        let minute = parts.minute ?? 0
        let hourText = hour < 10 ? "0" + String(hour) : String(hour)
        let minuteText = minute < 10 ? "0" + String(minute) : String(minute)
        let time = hourText + ":" + minuteText
        if calendar.isDate(date, inSameDayAs: now) { return "today " + time }
        if let yesterday = calendar.date(byAdding: .day, value: -1, to: now), calendar.isDate(date, inSameDayAs: yesterday) {
            return "yesterday " + time
        }
        let month = months[max(0, min(11, (parts.month ?? 1) - 1))]
        return month + " " + String(parts.day ?? 1)
    }
}
