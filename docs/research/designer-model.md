# Which model and CLI host for the Capstan "designer" role

Research date: 2026-10-03 (first pass and a second, Reddit-focused pass on the same day). Researcher: researcher-1. Question from the PM: which AI model, and which CLI agent host (Claude Code or Codex CLI), is best for UI and visual design work.

## Executive summary

**Recommendation: host `claude` (Claude Code), model `claude-opus-5-5`. Fallback: `claude-fable-5-1` (id now confirmed on an Anthropic page).** A second, Reddit-focused pass (2026-10-03) strengthened the first-pass recommendation and did not overturn it. Confidence is moderate to good.

Why:

- **Design preference benchmarks.** Design Arena (blind human votes on website generation) ranks Claude Opus 5.5 at 1342, third overall on 2026-10-02, the top model available on either host. Arena's Code Arena WebDev board (a second, independent crowd benchmark) had Opus 5.5 first by raw score on its 2026-09-23 snapshot: 1818 ±21 against GPT-6 Astra (Max) at 1792 ±12. The lead is within the confidence intervals, so treat it as a statistical tie at the top. Opus 5.5 also leads that board's Frontend (1848) and React (1851) views, per a secondary source.
- **Direct design comparisons.** Three independent September 2026 write-ups put Opus 5.5 ahead of Astra or Sol on taste and polish. The Reddit threads I read agree on taste (see Findings).
- **Cost.** Opus 5.5 lists at $4/$20 per million input/output tokens, against $10/$50 for Fable 5.1 and Astra.
- **Limits.** Several r/codex threads complain about how fast Astra burns Codex plan limits. Claude limits are also a recurring complaint, but Opus 5.5 is described as more usable.
- **Host.** Reddit users who prefer Astra's reasoning still keep Claude Code for its CLI and tooling.

Caveats the PM should weigh:

- **Astra is a real contender, not a clear loser.** It tops the Code Arena WebDev board as of Sep 11 and is within noise of Opus 5.5 on Sep 23. It is better on fine detail, persistence and hard architecture (Reddit, several posters). In Codex it also has built-in image generation (GPT Images 2.5) for a mockup-first workflow. If Capstan wants a Codex designer, `gpt-6-astra` is the model to try.
- **Claude's weak spot.** It is weaker on open-ended consumer UI and tends to over-fill pages with dense widgets (Claire Vo, September 2026).
- **GPT-6's weak spot.** It falls into repetitive aesthetic ruts (forest green palettes, geometric cards), and one r/codex poster found image-to-HTML fidelity poor.
- **Sonnet 5.5 is the cheap alternative.** One test put it second behind Opus 5.5 at about 30% lower cost, and one r/codex commenter found its output "pretty broken" on a 3D task. It is untested by any benchmark I found.
- **Reddit evidence is better than in the first pass but still anecdotal:** 7 threads read in their comments (partially), of which three are about visual output. See Method.

## Method

Date of research: 2026-10-03.

**Tools**

- WebSearch and WebFetch.
- The `www.reddit.com` Atom feeds through `curl`.
- Hacker News Algolia API.

**Queries and fetches**

- WebSearch queries covered:
  - best model for frontend UI design (Opus 5.5 vs Codex)
  - Design Arena leaderboard
  - Codex CLI model ids
  - GPT-6 Codex vs Opus 5.5 frontend (extended mode)
  - Codex vs Claude Code rate limits
  - Claude Design
  - Reddit sentiment on Opus 5/Astra
- Fetched pages: the articles listed under Sources.
- Reddit:
  - `r/ClaudeCode/search.rss?q=frontend+design+codex+opus&restrict_sr=1&sort=top&t=year` returned 20 thread titles and dates.
  - `r/ClaudeCode/comments/1wdtrsa/.rss` ("I'm done with Opus 5", Sep 11 2026) returned a 155 KB feed. I read the first ~15 KB, about 15 comments.
  - `r/webdev/search.rss?q=claude+codex+UI+design&restrict_sr=1&sort=top&t=year` returned 15 titles, none relevant to model choice for design.
- Hacker News: two Algolia queries returned no relevant stories for this question.

**Second pass (Reddit focus, same day).** Pacing that worked: one `www.reddit.com` request every 75 seconds, run from a background shell loop. Every request in the second pass returned 200, against about 1 in 5 in the first pass, which had no pacing.

- WebSearch with `site:reddit.com ...` queries (Opus 5.5 frontend design, GPT-6 Astra UI design codex, Claude Code vs Codex frontend UI taste) returned no Reddit pages, only blogs. So thread discovery went through the Reddit feeds instead:
  - `www.reddit.com/search.rss?q=astra+opus+frontend+design+UI&sort=top&t=month` (site-wide)
  - `r/codex/search.rss?q=astra+design+UI+frontend&restrict_sr=1&sort=top&t=month`
  - `r/ClaudeAI/search.rss?q=opus+5.5+design+UI+frontend&restrict_sr=1&sort=top&t=month`
  - `r/ChatGPTCoding/search.rss?q=astra+opus+frontend+design&restrict_sr=1&sort=top&t=month`
- Route for comments: `/r/<sub>/comments/<id>/.rss`, first ~9 KB of each feed (the post plus the first handful of comments). No Reddit mirror or archive was used.
- Web: Arena (formerly LMArena) Code Arena WebDev via search and secondary write-ups, benchlm.ai model pages, Anthropic's Fable page for the model id, and further comparison write-ups (MindStudio on Astra, Chase AI on Astra with GPT Images 2.5, Wiegold on Sonnet 5.5).

**Reddit threads actually read in their comments: 7** (each partially, the post plus the first comments, not the whole thread)

| # | Subreddit | Thread | Posted | About |
|---|---|---|---|---|
| 1 | r/ClaudeCode | "I'm done with Opus 5" (`1wdtrsa`) | 2026-09-11 | General agent quality; first pass |
| 2 | r/codex | "GPT Image 2.5 comparison for UI generation" (`1wb8p1g`) | 2026-09-09 | UI mockups by image model; design |
| 3 | r/codex | "Sol 6.1 vs Astra vs Opus 5.5. All Medium." (`1wtkoaf`) | 2026-09-29 | Same prompt, cinematic 3D scene; visual output |
| 4 | r/codex | "Astra vs Opus 5.5, my impressions on hard project" (`1wpveoe`) | 2026-09-25 | General coding, "artist vs engineer" |
| 5 | r/ClaudeAI | "I built my own Monarch-style finance dashboard with Opus 5.5 for less than $20" (`1wr140j`) | 2026-09-26 | React dashboard UI from screenshots |
| 6 | r/ClaudeAI | "Sonnet 5.5 did this. Opus 5.5 quality with half price." (`1wtagdd`) | 2026-09-29 | Code-generated motion video; cost |
| 7 | r/codex | "Opinion: Astra is overhyped" (`1wah1jk`) | 2026-09-08 | General; one-shot hype |

Three of the seven (#2, #3, #5) are about visual or UI output. Four are r/codex (Codex community), two r/ClaudeAI and one r/ClaudeCode, so both sides are represented.

**Reddit search listings (titles only, bodies not read): 6 listings, about 120 titles.** First pass: r/ClaudeCode and r/webdev. Second pass: site-wide, r/codex, r/ClaudeAI, r/ChatGPTCoding.

**Reddit content seen second-hand:** three threads through the botmonster article (r/ClaudeAI "Claude is BACK!", two r/codex threads) and one r/codex field report through tabbit.ai.

**Not reached:** r/OpenAI, r/Frontend, r/vibecoding, r/UI_Design and r/webdev beyond the title listing. No thread in these was read.

Reddit appears in the sources list as 7 threads plus the search listings. The recommendation rests mainly on benchmarks and published comparisons, supported by the threads.

## Findings

### Consensus (several independent sources agree)

1. **Claude leads on visual taste and polish in blind and head-to-head tests.**
   - Design Arena (benchlm, 2026-10-02) has Opus 5.5 at 1342 and Fable 5.1 at 1320. modelgrep (Sep 2026) has Opus 5.5 at 1360 on the same Elo system. GPT-5.5 is 31st.
   - Arena Code Arena WebDev (2026-09-23 snapshot, via aiidelist): Opus 5.5 (Max) 1818 ±21 and GPT-6 Astra (Max) 1792 ±12, a statistical tie. Fable 5.1 (Max) was 1758 and Opus 5 (Max) 1687 on the earlier 2026-09-11 snapshot (runtimewire), when Astra led at 1800.
   - Kilo (Apr 2026): Opus 4.7 produced more polished UIs on all five tasks than GPT-5.5, whose output read as generic SaaS templates.
   - MindStudio (Sep 24 2026): Opus 5.5 beat Astra on "premium" and layered visuals.
   - ChatPRD/Claire Vo (Sep 24 2026) and the podcast summary (Sep 27 2026): Opus 5.5 is strong for B2B and SaaS UI.
   - Lenny's Newsletter (Dec 3 2025): Opus 4.5 gave the polished redesign, GPT-5.1 Codex did not.
2. **Claude is strongest on dense B2B and dashboard UI and weaker on open-ended consumer apps.** Claire Vo's two write-ups agree, and uxmagic's recommendation matches.
3. **Spec faithfulness favours Claude on directed prompts.** Kilo: Opus 4.7 met all requirements except one overflow issue, while GPT-5.5 missed three (hero CTAs, save/cancel actions, login link). The podcast summary reports the same pattern for Opus 5.5 vs GPT-6 Sol on prompts with explicit requirements.
4. **Opus 5.5 is the cost-efficient Claude choice.**
   - Anthropic's launch post: $4/$20 per million tokens, 40% cheaper than Opus 5, over 30% faster.
   - Fable 5 and Astra are priced at $10/$50 (Anthropic; uxmagic; Cherickal).
5. **Workflow and harness matter more than model among frontier models.** uxmagic says "the tool you run them in, and the workflow around it, decides the result". The r/ClaudeCode commenter (Sep 11 2026) says only Claude's CLI and tooling keep them subscribed.
6. **Claude Code has a design-specific path.** Anthropic's Claude Design (Apr 17 2026, Opus 4.7) hands off a design bundle to Claude Code. Anthropic's frontend-design plugin/skill is also mentioned in a Nov 2025 r/ClaudeCode thread title and several blogs. I did not find a Codex equivalent in these sources.

### Isolated opinion (one source or one poster)

- **MindStudio (Sep 24 2026):** Astra finished a website test in 32 minutes vs 40 for Opus 5.5, and cost $11.33 vs $18.32. Single test, one author.
- **Cherickal (Sep 25 2026):** at maximum effort Opus 5.5 used about 119k output tokens per task vs Astra's 27k, making Astra cheaper per task despite higher per-token pricing.
- **uxmagic (Sep 25 2026):** rates Astra as having the "strongest visual judgment" for front-end design. It also lists Fable 5.1 as the most capable for long-horizon work.
- **Tensorlake (Feb 9 2026):** GPT-5.3 Codex produced a runnable UI in 3:53 vs 3:00 for Opus 4.6, with the Opus UI described as more carefully structured. This is an old-generation comparison.
- **r/ClaudeCode "I'm done with Opus 5" (Sep 11 2026, OP u/habfranco):** says Astra is far more intelligible than Opus 5 and Fable, while Claude's CLI and tooling are better. This predates Opus 5.5 and is not about design.
- **botmonster (Sep 25 2026), relaying Reddit:** users returned to Claude after Opus 5.5, with one r/codex $200 subscriber (437 votes) reporting a 20-hour Astra session used their monthly limit. One Sol 6 user defended it as cheap. This is a second-hand summary.
- **Claude Sonnet 5.5 and Haiku 4.5:** I found no design-specific evidence for either. Sonnet 5.5 was announced on 2026-09-28 (r/ClaudeCode title). Do not treat it as tested for design.

### Reddit (second pass): consensus vs isolated opinion

Quotes are short and attributed. All thread dates are 2026.

**Consensus across several threads and posters**

1. **Opus 5.5 is seen as the more tasteful, "artistic" model; the OpenAI models as the more careful "engineers".** In r/codex #4 (Sep 25), u/Dacadey writes that Opus and other Claude models "are the artists" while Astra and other ChatGPT models "are the engineers". Another commenter there (u/Chemical_Hawk_6307) says Opus "writes more tasteful code" but stops early. In r/codex #3 (Sep 29), u/digitalml, a paying ChatGPT $200 user, rates Opus 5.5 "by far the winner" on a cinematic Three.js scene. In the same thread, Astra's environment and UI design beat Sol 6.1's. This lines up with MindStudio and the podcast summary.
2. **Astra wins on depth and persistence, not on look.** u/muchsamurai (r/codex #4) says Astra at XHIGH is "still miles ahead" of Opus 5.5 on attention to detail and hard architecture. u/Chemical_Hawk_6307 says Astra persists until the issue is solved. These are about reasoning, not visual design.
3. **Codex plan limits are a common complaint.** r/codex #4: u/muchsamurai says Astra is "unusable" for coding on $100 plans, and uses a $200 Claude Max plan instead (60% of the weekly limit in 2 days). The r/codex search listing carries titles on the same theme (for example "Astra (Light) consistently burns through 100% of my 20x weekly limit every 12-16 hours", Sep 9). The botmonster article, second-hand, reports similar posts.
4. **Claude Code's harness is preferred.** r/ClaudeCode #1 (Sep 11): u/habfranco says only Claude's "CLI/tooling/harness" keeps their Claude subscription, despite preferring Astra's intelligence at that time (Opus 5, before 5.5).
5. **Both sides see the models as complementary.** Several r/codex commenters (#4) use Opus as executor and Astra as reviewer or architect.

**Isolated opinion (one poster or one thread)**

- r/codex #3: u/digitalml notes Opus 5.5 took 38m54s against 8m42s (Sol 6.1) and 9m37s (Astra), and later said they might downgrade ChatGPT from $200 to $20 and move to Claude. One test, one prompt, a 3D scene, not a UI.
- r/codex #3: another commenter (u/Tank_Gloomy) ran Sonnet 5.5 on the same prompt: "pretty broken" in about 7 minutes in chat mode. One run.
- r/ClaudeAI #5: u/RallyMantis built a React finance dashboard from researched screenshots with Opus 5.5 at medium effort, using 35% of a weekly Claude Pro limit, and "never felt the need to bump up reasoning". The mod-bot summary reports 200 comments of enthusiasm and no complaint about the UI. A single project, self-reported.
- r/ClaudeAI #6: u/oxmannnn built a code-generated 30-second motion video with Sonnet 5.5 (at API list prices $35.40 against $50.48 at Opus 5.5 prices). The mod-bot summary says the top comments called the result "chaotic" and "junior level", including a professional motion designer, and the cost comparison was disputed. Evidence that Sonnet 5.5 output can look weak on polish, but it is a motion video, not a UI.
- r/codex #2: u/withmagi, who builds a UI-generation product, says GPT Image 2.5 is the first image model to improve on every UI test. u/rc225225 replied that HTML built from the image "looks nothing like it". u/withmagi suggests asking Codex via `/goal` to keep comparing against the source image and to zoom into sections. This is about image-to-code fidelity, one commenter's experience.
- r/codex #7: u/UnderstandingDry1256 says Astra misses "high level intent" on real tasks and the UI still did not work. One poster.
- r/codex field report (second-hand via tabbit.ai, Sep 8): one user found the Light effort tier suited UI work and XHigh gave no measurable benefit for their implementation task. Second-hand, not read at source.
- Titles only, not read: r/ClaudeAI threads on Opus 5.5 possibly being "nerfed" ("Is Opus 5.5 entering a 'nerfed' phase? LiveNerf baseline update", Sep 29; "Opus 5.5 nerfing - how to measure, how to spot, how to sue", Oct 1), and r/codex "Quality degradation" and "Is OpenAI silently degrading models for selected users?". Both sides have degradation worries, and I did not verify either.

**What Reddit did not give me:** no thread in my sample compared models on responsive layout, accessibility, or typography, and none was about a Claude-vs-Codex design workflow in a real product. The Reddit evidence on pure design taste comes from threads #2, #3 and #5.

### Astra in Codex has built-in image generation

Chase AI (Sep 9) describes a Codex workflow where Astra uses built-in GPT Images 2.5 to generate reference mockups and design variations (20 landing page variations), then refines in code. The author says baseline Astra "held up against" popular design skills with no skill installed. Anthropic models need an external image tool (Fal or Higgsfield), per the same author. This is a real Codex advantage for mockup-first design, but r/codex #2 reports that image-to-HTML fidelity is a weak step. One author and one commenter, so isolated.

### Codex CLI model ids

From the Codex models documentation (learn.chatgpt.com, fetched through WebFetch, which summarises with a small model, so verify with `/model` in the CLI):

- `gpt-6-astra`: most capable.
- `gpt-6.1-sol`: recommended for repeated work.
- `gpt-6-luna`: fastest, for clear tasks.
- `gpt-5.5`: retiring on 2026-10-14.

Reasoning effort runs from Light to Ultra via `/model`.

### Limits and cost by host

- **Claude Code:** one pool shared with Claude chat, with rolling 5-hour windows and weekly caps (Cherickal; morphllm snippet: Pro $20, Max 5x $100, Max 20x $200).
- **Codex:** bundled with ChatGPT plans, Plus about $20, Pro 5x $100, Pro 20x $200, with separate metering from Chat. Seawork (Jul 14 2026) reports Plus message ranges per 5-hour window by tier: Sol 15 to 90, Luna 50 to 280. Those figures are for the GPT-5.6 family and are dated.
- **Per-token API prices:** Opus 5.5 $4/$20, Fable 5.1 $10/$50, Astra $10/$50, GPT-6 Sol $2/$10, Claude Sonnet 5 $2/$10 (uxmagic). Sonnet 5.5 pricing is not verified.

## Comparison table

Ratings are my synthesis of the cited sources, not measured by me. "n/e" means no design evidence found.

| Candidate | Host | Model id | Visual taste / polish | Layout, type | Spec faithfulness | Responsive / a11y | Speed | Cost (API per M tokens) | Notes |
|---|---|---|---|---|---|---|---|---|---|
| Claude Opus 5.5 | claude | `claude-opus-5-5` | Best available: Design Arena 1342 (3rd overall, top on either host) | Strong, dense B2B/dashboard; weak on consumer | Strong on directed prompts (Kilo/podcast, older Opus versions) | n/e | 30%+ faster than Opus 5; 40 min vs 32 for Astra in one test | $4 / $20 | Recommended |
| Claude Fable 5.1 | claude | `claude-fable-5-1` (confirmed, released 2026-09-01) | Design Arena 1320; Code Arena WebDev 1758 (Sep 11) | Good | n/e | n/e | Slower, long-horizon strength | $10 / $50 | Fallback for complex tasks; the Fable 5 launch post says safeguards route some requests to Opus 4.8 |
| Claude Sonnet 5.5 | claude | `claude-sonnet-5-5` | Second to Opus 5.5 in one web design test, "close" (Wiegold); "pretty broken" on one r/codex 3D test | Comparable (one test) | n/e | Opus 5.5 had the more careful accessibility work in the same test | One r/codex run took ~7 min in chat mode | About $2 / $10 (poster-stated) | Cheaper (about 30% less in one test); no benchmark entry found |
| Claude Haiku 4.5 | claude | `claude-haiku-4-5-20251001` | Design Arena rank 74 of 91 (1129) | n/e | n/e | n/e | Fast (small model) | not verified | Not recommended for taste work |
| Claude Opus 5 | claude | n/a | Design Arena 1316 | n/e | n/e | n/e | slower, 40% costlier than 5.5 | n/e | Community complaints about its communication (r/ClaudeCode Sep 2026) |
| Claude Opus 4.7 / 4.6 | claude | n/a | Opus 4.7 beat GPT-5.5 in Kilo; Design Arena 1320 / 1298 | Varied type and density | Good | n/e | n/e | n/e | Older predecessors |
| GPT-6 Astra | codex | `gpt-6-astra` | Code Arena WebDev 1792 (Sep 23), tied with Opus 5.5; less polished than Opus 5.5 in MindStudio's test; "strongest visual judgment" per uxmagic; Reddit: Astra's UI beat Sol's, Opus 5.5 beat both; built-in image generation in Codex | Repetitive palettes and card layouts (GPT-6 Sol per the podcast summary) | n/e directly | n/e | Faster in one test | $10 / $50 | Fewer tokens per task in two sources |
| GPT-6.1 Sol | codex | `gpt-6.1-sol` | n/e directly; Sol falls into repetitive ruts per podcast summary | same | Weaker than Opus 5.5 on explicit requirements | n/e | n/e | $2 / $10 (for "GPT-6 Sol") | Cheap |
| GPT-6 Luna | codex | `gpt-6-luna` | n/e | n/e | n/e | n/e | Fastest | n/e | Not design-tested |
| GPT-5.5 | codex | `gpt-5.5` | 31st on Design Arena (1264); lost to Opus 4.7 in Kilo | Generic SaaS look | Missed 3 of requirements in Kilo | n/e | Fewer tokens | $30/M output (Kilo) | Retiring 2026-10-14 |

Responsive and accessibility handling: none of the sources I read measured it per model. Treat that dimension as unevidenced for every candidate.

## Conflicts and disagreements

1. **Astra vs Opus 5.5 for front-end.**
   - The first-pass "Astra leads the Frontend Code Arena (1793)" claim traces to Arena's Code Arena: WebDev board, not to uxmagic. Arena posts on X (cited in search results) say Astra (Max) was #1 at about 1797 to 1800 points. The runtimewire report is dated to the 2026-09-11 snapshot: Astra 1800, Fable 5.1 1758, Opus 5 1687.
   - That claim is stale. On the 2026-09-23 snapshot, Opus 5.5 (Max) leads by raw score, 1818 ±21 vs Astra 1792 ±12, and the intervals overlap. Opus 5.5 was released on 2026-09-22.
   - Design Arena (taste) and MindStudio, ChatPRD and the podcast summary prefer Opus 5.5. uxmagic calls Astra's visual judgment strongest, and its page also says no public Frontend Code Arena leaderboard existed when it was written.
   - Net: Astra vs Opus 5.5 is a close call on web-app preference and Opus 5.5 leads on taste-focused sources.
2. **Opus 5.5's Design Arena score:** 1342 (benchlm, 2026-10-02) vs 1360 (modelgrep, Sep 2026) vs a search snippet saying 1385. The snapshots are from different dates. All agree it is the top Claude model.
3. **Leaderboard leaders.** Muse Spark 1.3 (1364) and Kimi K3 (1344) are above Opus 5.5 on Design Arena. Neither runs in Claude Code or Codex CLI, so they do not affect the host decision.
4. **Cost.** Per token, Astra is about 2.5 times Opus 5.5. Per task, MindStudio and Cherickal found Astra cheaper because it used fewer tokens. Kilo found the same for GPT-5.5 ($2.65 vs $5.07) while Opus 4.7 produced better UIs. So the cost winner depends on what is measured.
5. **Model naming.** Seawork describes "GPT-5.6" Sol, Terra and Luna on Codex, while the Codex docs list `gpt-6-astra`, `gpt-6.1-sol` and `gpt-6-luna`. I could not reconcile these.
6. **Astra speed.** MindStudio (32 vs 40 minutes) and r/codex #3 (about 9.5 vs 39 minutes) both found Astra faster than Opus 5.5 on one test each. Reddit #4 poster and the tabbit field report describe Astra as slow and limit-hungry on long agentic tasks. The two views are about different workloads, and none was measured on UI iteration.
7. **Reddit vs benchmarks on Astra's strength.** Reddit posters say Astra is stronger on depth and detail, while the design-taste benchmarks and write-ups favour Opus 5.5. Code Arena WebDev, which tests working web apps, is a tie. These are different questions (reasoning vs look), so I do not treat them as contradictory.
8. **Reddit sentiment on Opus 5 vs Astra.** The Sep 11 thread leans toward Astra's reasoning, while botmonster's later roundup reports a move back to Claude after Opus 5.5. These are two different Claude versions, so the sources are consistent.

## Suggested configuration (snippet only, for the doc)

Keys follow the README: `kind`, `host`, `model`, `permission_mode`, `allow`, `deny`, `prompt`.

```toml
[roles.designer]
kind = "Developer"
host = "claude"
model = "claude-opus-5-5"
permission_mode = "acceptEdits"
deny = ["Bash(git push)", "Bash(git push *)"]
prompt = "You are the designer. Work from the spec or mockup you are given and follow its layout, typography and copy exactly. Pick a deliberate aesthetic direction before coding, avoid generic defaults, and handle responsive widths and accessibility (contrast, focus states, semantic markup). Check the result visually before you report."
```

Fallback, same role with `model = "claude-fable-5-1"`. Use it for complex, long-horizon design work, at about 2.5 times the token price.

If a Codex-hosted designer is wanted (for example, to diversify or when Claude limits run out), the README requires its own model, `acceptEdits` or `auto`, and no `allow` or `deny`:

```toml
[roles.designer-codex]
kind = "Developer"
host = "codex"
model = "gpt-6-astra"
permission_mode = "acceptEdits"
prompt = "..."
```

Codex roles run unsandboxed (README), which is a reason to prefer the Claude host for this role.

## Sources

Access date for every entry is 2026-10-03. Dates are publication or post dates as stated by the page.

Reddit
1. "I'm done with Opus 5", r/ClaudeCode, u/habfranco and commenters. https://www.reddit.com/r/ClaudeCode/comments/1wdtrsa/im_done_with_opus_5/ . Posted 2026-09-11. Comments feed read in part.
2. r/ClaudeCode search results for "frontend design codex opus" (top, last year). https://www.reddit.com/r/ClaudeCode/search.rss?q=frontend+design+codex+opus&restrict_sr=1&sort=top&t=year . Titles dated 2025-10 to 2026-09. Titles only, bodies not read, apart from the Fable 5 announcement post (2026-06-09), which carries the Fable 5 details in the feed.
3. r/webdev search results for "claude codex UI design". https://www.reddit.com/r/webdev/search.rss?q=claude+codex+UI+design&restrict_sr=1&sort=top&t=year . Titles dated 2025-10 to 2026-09. Titles only. Nothing relevant.

Reddit, second pass (comments read in part, first ~9 KB of each feed)
- r/codex, "GPT Image 2.5 comparison for UI generation", u/withmagi and commenters. https://www.reddit.com/r/codex/comments/1wb8p1g/gpt_image_25_comparison_for_ui_generation/ . 2026-09-09.
- r/codex, "Sol 6.1 vs Astra vs Opus 5.5. All Medium.", u/digitalml and commenters. https://www.reddit.com/r/codex/comments/1wtkoaf/sol_61_vs_astra_vs_opus_55_all_medium/ . 2026-09-29.
- r/codex, "Astra vs Opus 5.5, my impressions on hard project", u/muchsamurai and commenters. https://www.reddit.com/r/codex/comments/1wpveoe/astra_vs_opus_55_my_impressions_on_hard_project/ . 2026-09-25.
- r/ClaudeAI, "I built my own Monarch-style finance dashboard with Opus 5.5 for less than $20", u/RallyMantis. https://www.reddit.com/r/ClaudeAI/comments/1wr140j/i_built_my_own_monarchstyle_finance_dashboard/ . 2026-09-26.
- r/ClaudeAI, "Sonnet 5.5 did this. Opus 5.5 quality with half price.", u/oxmannnn. https://www.reddit.com/r/ClaudeAI/comments/1wtagdd/sonnet_55_did_this_opus_55_quality_with_half_price/ . 2026-09-29.
- r/codex, "Opinion: Astra is overhyped", u/UnderstandingDry1256 and commenters. https://www.reddit.com/r/codex/comments/1wah1jk/opinion_astra_is_overhyped/ . 2026-09-08.
- Reddit search feeds used for discovery (titles only): site-wide `search.rss?q=astra+opus+frontend+design+UI&sort=top&t=month`; r/codex, r/ClaudeAI and r/ChatGPTCoding `search.rss` queries listed under Method. Titles dated 2026-09 to 2026-10.
- r/codex field report "100k LOC long horizon effort", u/PublicReality4401, 2026-09-08, seen second-hand at https://go.tabbit.ai/model/gpt-6-astra/reviews/reddit-codex-100k-loc-long-horizon-effort-field-report (not read at source).

Further web sources, second pass
- "Claude Opus 5.5 Hits #1 in Code Arena WebDev: What Its 1818 Score Really Means", aiidelist.com. https://aiidelist.com/blog/claude-opus-5-5-code-arena-webdev . Reports the 2026-09-23 Arena snapshot.
- "Arena ranks GPT-6 Astra Max first for web development, Claude Fable 5.1 Max first for agents", runtimewire.com. https://runtimewire.com/article/arena-gpt-6-astra-webdev-leaderboard-claude-agents . Reports the 2026-09-11 snapshot.
- "GPT-6 Astra Design Quality", Luis Chavez-Mattos, MindStudio. https://www.mindstudio.ai/blog/gpt-6-astra-design-websites . 2026-09-05.
- "Astra front-end design", Chase Hannegan, Chase AI. https://chaseai.io/blog/gpt-astra-front-end-design . 2026-09-09.
- "Claude web design test, September 2026", Thomas Wiegold. https://thomas-wiegold.com/blog/claude-sonnet-5-5-review/web-design-test/ . September 2026.
- Claude Fable 5.1 page, Anthropic. https://www.anthropic.com/claude/fable . Gives `claude-fable-5-1`, released 2026-09-01.

Benchmarks
4. Design Arena website Elo, benchlm.ai. https://benchlm.ai/benchmarks/designArenaWebsite . Page dated 2026-10-02.
5. Best Anthropic models for design, modelgrep.com. https://modelgrep.com/best/design/anthropic . September 2026.
6. "DesignArena creators raise $7.9 million to bring taste to AI models", Russell Brandom, TechCrunch. https://techcrunch.com/2026/08/03/designarena-creators-raise-7-9-million-to-bring-taste-to-ai-models/ . 2026-08-03. Explains how the benchmark's votes are collected.

Comparisons and reviews
7. "We Asked GPT-5.5 and Claude Opus 4.7 to Design 5 UIs", Darko Gjorgjievski, Kilo Blog. https://blog.kilo.ai/p/we-asked-gpt-55-and-claude-opus-47 . 2026-04-27.
8. "Claude Opus 5.5 review", Claire Vo, ChatPRD (How I AI). https://www.chatprd.ai/how-i-ai/claude-opus-5-5-review . 2026-09-24.
9. "Benchmarking Frontend Code: Claude Opus 5.5 vs. GPT-6", How I AI via The Podcast Summary. https://www.thepodcastsummary.com/episodes/LMT-bknLmNo/evaluating-frontend-ui-prototypes-claude-vs-openai.html . 2026-09-27.
10. "Claude Opus 5.5 vs GPT-6 Astra", Luis Chavez-Mattos, MindStudio. https://www.mindstudio.ai/blog/opus-5-5-vs-gpt-6-astra/ . 2026-09-24.
11. "Best AI Model for UI Design (2026): Claude, GPT, Gemini", Ajay Khatri, uxmagic.ai. https://uxmagic.ai/blog/best-ai-model-for-ui-design . 2026-09-25.
12. "Opus 5.5 is the Claude comeback Reddit was waiting for", botmonster.com. https://botmonster.com/ai/opus-5-5-is-the-claude-comeback-reddit-was-waiting-for/ . 2026-09-25. Second-hand Reddit summary.
13. "ChatGPT-6 versus Claude 5.5 - The Ultimate Comparison", Thomas Cherickal. https://thomascherickal.com/2026/09/25/chatgpt-versus-claude-the-ultimate-comparison/ . 2026-09-25.
14. "Claude Opus 4.6 vs GPT 5.3 Codex", Tensorlake team. https://tensorlake.ai/blog/claude-opus-4-6-vs-gpt-5-3-codex . 2026-02-09.
15. "Which AI model is the best designer", Claire Vo, Lenny's Newsletter. https://www.lennysnewsletter.com/p/which-ai-model-is-the-best-designer . 2025-12-03.

Vendor and docs
16. "Claude Opus 5.5", Anthropic. https://www.anthropic.com/news/claude-opus-5-5 . 2026-09-22.
17. "Claude Fable 5 and Mythos 5", Anthropic. https://www.anthropic.com/news/claude-fable-5-mythos-5 . 2026-06-09.
18. "Introducing Claude Design by Anthropic Labs", Anthropic. https://www.anthropic.com/news/claude-design-anthropic-labs . 2026-04-17.
19. Codex models documentation (developers.openai.com/codex/models.md redirects here). https://learn.chatgpt.com/docs/models.md . Undated. Fetched 2026-10-03.
20. "Codex Pricing (2026)", SeaWork. https://seawork.ai/en/blogs/codex-pricing/ . 2026-07-14.

Search result snippets used only as pointers (not relied on): morphllm.com "Claude Code vs Codex pricing and limits" (June 2026) and "Best AI model for coding (September 2026)", both unfetchable.

## Could not verify

- **First pass Reddit failures.** Unpaced requests got 429 about four times in five (`x-ratelimit-remaining: 0`). The second pass, paced at 75 seconds, got 200 every time.
- **Reddit reads are partial.** Seven threads were read in their comments feeds, but only the first ~9 KB of each (the post and the first handful of comments), because the feeds are large (one was 155 KB). Later comments, which may disagree, were not read. Mod-bot "TL;DR" comments summarise 200 comments in two threads, and I relied on those summaries only for the points marked as such.
- **Reddit gaps.** I did not read any thread from r/OpenAI, r/Frontend, r/vibecoding, r/UI_Design or r/ChatGPTCoding. For r/ChatGPTCoding I saw only titles. `site:reddit.com` WebSearch returns no Reddit pages, so threads were found through Reddit's own search feeds, which rank by votes, not by relevance to design. Only three of the seven threads concern visual output, and none covers responsive layout, accessibility or typography.
- **No Reddit mirror or archive** was tried beyond the sentinel-team snapshot URLs that appeared in search results (not opened).
- **Reddit "nerfing" claims** (Opus 5.5 and Codex quality degradation) are from titles only and were not checked.
- **Hacker News:** two Algolia queries returned nothing relevant. X was not tried; an Arena post on X returned 402.
- **Pages that blocked me:** openai.com/index/gpt-5-6 (403), uxplanet.org GPT-6 Astra vs Opus 5 (403, member-only), morphllm.com (429), anthropic.com/news/claude-sonnet-5-5 (404). The Design Arena leaderboard page loaded but its table was not in the content returned.
- **Code Arena WebDev scores** (Astra 1792 to 1800, Opus 5.5 1818, Fable 5.1 1758) come from search-result summaries and two secondary articles (runtimewire, aiidelist), not from the Arena leaderboard itself: arena.ai/leaderboard/webdev returned "Leaderboard Not Found" and the Arena X post returned 402. The Opus 5.5 Frontend (1848) and React (1851) figures are from one secondary source. The 1793 figure is close to, but not exactly, any number I found (1792, 1797, 1800).
- **GPT-6 Astra, Sol and Luna on Design Arena:** not on the benchlm board I read (91 models), and benchlm's Astra and Sol model pages list no design or WebDev benchmark. Sol 6.1 and Luna have no design benchmark entry that I found. Astra's only crowd-benchmark result is Code Arena WebDev.
- **Codex model ids** come from a summary produced by the fetch tool, not from running `codex`. Confirm with `/model` or `codex --model` before configuring. The `gpt-5.5` retirement date (2026-10-14) is from the same page.
- **Model-name conflict:** Seawork's GPT-5.6 Sol/Terra/Luna vs the docs' `gpt-6-astra`, `gpt-6.1-sol`, `gpt-6-luna`.
- **Claude Sonnet 5.5 and Haiku 4.5:** no design evidence, and no verified Sonnet 5.5 pricing. The model ids `claude-sonnet-5-5` and `claude-haiku-4-5-20251001` come from the session environment, not from a vendor page I fetched. `claude-opus-5-5` is confirmed by Anthropic's page. `claude-fable-5-1` is now confirmed: Anthropic's Fable page (fetched through WebFetch) gives it as the API id, released 2026-09-01 at $10/$50, and the platform.claude.com model docs URL path `models/fable-5-1` and a search summary agree. `claude-fable-5` is the earlier Fable 5 id.
- **Responsive and accessibility handling and iteration speed** were not measured per model by any source I read. The speed and cost data points are single tests.
- **Rate limits** figures come from third-party blogs and search snippets, not from vendor pricing pages, and may be stale.
