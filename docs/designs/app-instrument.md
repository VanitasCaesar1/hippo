# Hippo.app: the instrument look

Status: accepted 18 Nov 2026, to build on Fleet Pulse ([fleet-pulse.md](fleet-pulse.md)). Covers the app's look, its sidebar, Home and Pulse. Amended 18 Nov 2026: Rows, Lanes, Inbox, Incidents and Memory take the look too (the first build left them with their badges, icons and cards, beside Pulse). The session views (the thread, the x-ray) and Manage keep their layout and take the new look from the shared tokens and kit.

## Background

Two complaints about Hippo.app, from using it: it is cluttered, with many things to see and process at once, and it looks generic, like any dashboard. Home showed the waiting sessions twice (its list and the sidebar's), a headroom card, a memory card, an overnight line of facts joined by dots, note-change chips and a list of corrections, beside a sidebar with five counts and every session of the day. Every row, header and button carried an icon; key hints sat on most controls; titles and reasons were cut to one line; the same thing was called an agent, a session and a thread. Counts disagreed between views because each counted by its own rule.

Three mocks were made. A calm one (fewer things, sentences) was calmer but generic. A warm one (paper colours, a serif) was rejected as another look every AI product has. The third, an instrument, was accepted: the app reads like a well-made piece of equipment for people who run agents.

## What changes for a user

### The look

- **Neutral, not blue.** Near-black and white surfaces with no tint. Colour appears only for a state: amber for waiting on you, red for harm and for an account that runs dry, green for checked and passing. The account colours (violet, pink) stay on account marks and meters only. The theme picker's other themes are unchanged.
- **Two typefaces, both the system's.** SF Pro for what people and agents wrote (titles, questions, reasons, notes). SF Mono for every number, time, count, command and label. Labels are SF Mono in capitals at 10.5 points with wide tracking, in the faint colour. Numbers are tabular.
- **Hairlines and a grid, no cards.** Areas are divided by 1-point lines; lists are rows divided by fainter lines; corners are square except buttons (3 points). Nothing sits in a box inside a box.
- **Marks, not icons.** One mark per state, an 8-point square: filled amber (waiting on you, blinking unless the system asks for reduced motion), filled red (harm), hollow red (runs dry), hollow grey (a note: an unverified claim, a rule to keep). Kept rules keep their ring where Lanes draws them. Rows, headers and buttons carry no other icon.
- **Keys on buttons only.** A focused item's buttons show their key in a faint label inside the button (Open thread ↵, T3 Code T, Terminal ⌘T, Later S). No other control shows a key hint; the queue's hint line is gone.
- **One word.** A run of an agent is a *session* everywhere the app speaks; *thread* stays only in *Open thread*, which opens the app's thread view.
- **Counts agree.** *Working* means sessions working now ([`Row::working`]) wherever it appears: Home, Pulse, the sheet and the menu-bar item.

### The sidebar

Navigation only: Home, Inbox, Fleet, Incidents, Memory, with the selected one marked by a 2-point bar at its left edge. Only Inbox shows a count, because only it asks for something. The day's sessions are no longer listed there; they are in Fleet. The foot says *INDEXED 16:34*; it turns red and says what failed only when ingest has failed.

### Home

One column, at most 800 points wide:

- **The date** as a label.
- **Readouts.** Four cells in a row: *Waiting on you* (the count, and the oldest wait), *Working* (the count, and sub-agents), and each account's share of its 5-hour window left (the figure, and *empty 16:48* in red when it runs dry before its reset, else its reset time). A large SF Mono figure under each label.
- **The scope band**: the fleet's tool calls in 2-minute beats over the last 3 hours, with a time scale and the current beat lit at *NOW*.
- **Waiting on you**: the sessions that wait (a question or a plan), then an account that runs dry, each a row with its mark, title, age and one line of reason. A row opens Pulse on that item.
- **Last night**: label and value rows. *Read*, the sessions the last nightly pass read; *Notes*, how many it updated and that a fresh reviewer confirmed each, with *Open*; *Rules*, how many lessons it proposed, with *Review* (Inbox); *Tonight*, when the next pass runs and how many sessions it will read.
- **The prompt** at the foot: Ask or Brief, as today, drawn as a command line (`›`).

Gone from Home: the headroom card (its figures are the readouts), the memory card, the overnight line, the note-change chips and the list of corrections (they are in Inbox).

### Pulse

Pulse's content and keys are as [fleet-pulse.md](fleet-pulse.md) describes; this is how it is drawn.

- **Header.** *Fleet*, the switcher (*PULSE LANES ROWS* as labels, the selected one underlined), then on the right the readouts *WAITING 02* and *WORKING 05* and each account's meter: its name, a bar of 10 segments for the share left (red when it runs dry, else the account's colour), the figure, and *T−14M* in red when it runs dry. The span picker and the account filter leave Pulse; Lanes keeps its own.
- **The scope band** under the header, across the full width, as on Home.
- **Queue.** Two groups, *WAITING ON YOU* and *COMING UP*, each with its count as a label. A row is a mark, a title, an age (SF Mono, right) and a reason of up to two lines. The selected row has the 2-point bar. For the Next up kinds hippo writes the title: *default runs dry at 16:48*, *A force push lost 2 commits*, *A rule worth keeping*, the session's own title for an unverified claim. No account mark on rows. The foot: *WORKING 05 SUB-AGENTS 07 FLEET →* opens the sheet.
- **Focus.** One column of at most 680 points, left-aligned with a 48-point gutter.
  - *A waiting session*: its state as a label with a running timer since it asked (*WAITING 10:12*, counting each second), its project and account as labels at the right; its title; the question in 17 points with an amber rule at its left; the buttons; then a table of label and value rows: *Branch*, *Pull request* (number and diff, with *CHECKS PASS* or *FAILING* at the right), *Tests* or the claims (*CHECKED* or *UNVERIFIED*), *Started with* (the kept rules, each on its line). Rows without a value are left out. Then *LAST CALLS*: the last three, time, call (commands in SF Mono) and duration or diff, with *+ N EARLIER* to show the rest.
  - *A plan waiting for approval*: the steps as numbered rows (*01*, *02*), then the buttons.
  - *An account that runs dry*: *RUNS DRY*, a countdown to the empty time in large red SF Mono (*T−14:00*), the title and one line (*27 minutes before it resets. Three sessions on it will stop.*); the drain chart: the share used from the window's start to now as a line in the account's colour, the projection to 100% dashed in red, a now line, a square at the empty time, and a grid at 25% steps and hours; then rows *Left*, *Rate*, *Resets*, *Will stop* (each session); then *Resume them on work*.
  - *An unverified claim, a destructive command, a rule to keep*: the claim quoted with a grey rule, or the command and its output in a block; rows for the evidence (*Typecheck: none ran*, *Last command*; *Overwrote*, *Corrected*, *Now*; each correction with its time and session, *Goes into* the note); one button.
- **The sheet** (`F`): *WORKING 05* and *CLOSE*, then each working session as a row: its title, its current call in SF Mono, and its last 30 minutes as 15 bars in its account's colour. The foot: *TO REVIEW 03 QUIET 02 SETTLED 24*, each opening Rows. Memory today and Tonight move to Home.
- **All clear**: the readouts of the fleet's day (*TOOL CALLS 5 MIN*, *AGENT WORK TODAY*, *LINES +2,184 −611*, *TONIGHT 03:00*) over the scope band for the whole day.

### Rows, Lanes, Inbox, Incidents and Memory

Each keeps what it shows and how it is laid out; it is drawn with the same parts as Pulse.

- **Headers.** A view's title, then its switches as labels with the selected one underlined (*PULSE LANES ROWS*; Rows' *STATE PROJECT ACCOUNT*; Lanes' *DAY 6H 1H*; Inbox's *ALL CORRECTIONS FAILURES NOTES*, each with its count). No segmented boxes.
- **Accounts.** An account is its name as a label (*WORK*), with a 2-point bar in its colour where a row needs to say which account it ran on (a lane, a Rows row). The letter badges (*De*, *Wo*) are gone. The account filter is the accounts' names as labels.
- **Groups and columns.** Group headings and column heads are labels with their count (*NEEDS YOU 02*); no badges and no chevrons (a group still folds when its heading is clicked).
- **Rows.** A row is a mark, its text and its values in SF Mono at the right: state and time (*WAITING 13M*, *WORKING 2H 32M*, *QUIET 13:42*), claims (*1 CHECKED*, *1 UNVERIFIED*), pull requests (*#57 OPEN*), branch. A waiting row's *Open thread* stays; no icon in any cell.
- **Lanes.** The day's line becomes readouts (*SESSIONS*, *SUB-AGENTS*, *AGENT WORK*, *WAITED ON YOU*, *INCIDENTS*); the legend shows the marks, not icons; marks on lanes are the vector marks of [fleet-pulse.md](fleet-pulse.md) (ring, diamond, square, dot) and the amber square for a question. *At this minute* is a row on a hairline at the foot, not a card.
- **Inbox, Incidents, Memory.** Lists are rows on hairlines; facts are spec tables (label and value rows); a quote has a grey rule at its left as in Pulse; cards, chips and note icons become rows. An empty pane says what to do in one line, with no icon.

## What the engine adds

Nothing new is read or written. The titles hippo writes for Next up are made in the app from `pulse::Next`. The scope band reads `replay::Track::beats`, summed across tracks. The timer counts from `State::Waiting::at`.

## The app

- The theme files `hippo-dark.json` and `hippo-light.json` take the new values. Dark: background `#0C0C0C`, panel `#090909`, surface `#131313`, hover `#151515`, selected `#1B1B1B`, text `#EDEDED`, muted `#A3A3A3`, faint `#7A7A7A`, borders `#FFFFFF14` and `#FFFFFF26`, warning `#FFB224`, error `#FF5A4F`, success `#3DD68C`; the accent is the text colour. Light: background `#FAFAFA`, panel `#F3F3F3`, surface `#FFFFFF`, hover `#F0F0F0`, selected `#E9E9E9`, text `#111111`, muted `#505050`, faint `#6B6B6B`, borders `#0000001A` and `#00000033`, warning `#B45309`, error `#C8102E`, success `#15803D`. Every text pair keeps 4.5:1 and every control 3:1.
- The kit gains a label (SF Mono, capitals, tracked), a readout (label over a large figure), a segmented meter, a spec table (label and value rows with an optional status at the right), the square marks and the scope band; buttons take square corners and an optional key label. Views use these instead of icons and pills.
- Below about 900 points Pulse stacks as before, and Home's readouts wrap two to a row.

## Alternatives

- **Calm, with sentences.** Fewer things and plain sentences for facts. Calmer, but generic: a blue-black dashboard like many others, and sentences hide numbers people scan for.
- **Warm paper and a serif.** An editorial look. Rejected: it is the look many AI products share now.
- **Keep the sidebar's sessions.** They repeat Fleet and the queue, and the sidebar's Working shelf (live and quiet) contradicted the count of sessions working now.
