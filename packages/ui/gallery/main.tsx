import { render } from "@solidjs/web";
import { createSignal, For, Show, untrack } from "solid-js";
import "../src/styles.css";
import "./gallery.css";
import {
  Button,
  Checkbox,
  ConfirmDialog,
  Diff,
  IconButton,
  ListRow,
  Menu,
  NumberField,
  Select,
  StatusDot,
  Tabs,
  TextField,
  type DiffLineAnnotation,
  type Status,
} from "../src";
import { drafts, newFile, oldFile, type DraftComment } from "./fixtures";
import { BotIcon, ChatIcon, EllipsisIcon, ExternalIcon, FilterIcon, PanelRightIcon, PullRequestIcon, SearchIcon } from "../src";

// The gallery shows every component with invented data. `?dialog` opens the dialog at load, for screenshots.
const params = new URLSearchParams(location.search);

function Sidebar() {
  const [selected, setSelected] = createSignal("w101");
  const row = (id: string) => ({ selected: selected() === id, onClick: () => setSelected(id) });
  return (
    <aside class="gallery-sidebar" aria-label="All work">
      <div class="gallery-sidebar-toolbar">
        <h2>All work</h2>
        <IconButton label="Search">
          <SearchIcon />
        </IconButton>
        <IconButton label="Filter">
          <FilterIcon />
        </IconButton>
      </div>
      <h3 class="gallery-group">Needs you</h3>
      <ul class="gallery-rows">
        <li>
          <ListRow icon={<PullRequestIcon />} title="separate the cache configuration" count={2} status="attention" meta="widgets#101 · approve once CI passes" {...row("w101")} />
        </li>
        <li>
          <ListRow icon={<ChatIcon />} title="Score threshold cleanup" status="attention" meta="Codex · waiting for your approval" {...row("chat1")} />
        </li>
        <li>
          <ListRow icon={<PullRequestIcon />} title="add the tags package with Tag and List types" count={1} status="error" meta="widgets#107 · review failed" {...row("w107")} />
        </li>
      </ul>
      <h3 class="gallery-group">acme/widgets</h3>
      <ul class="gallery-rows">
        <li>
          <ListRow icon={<ChatIcon />} title="Saved results for queries" status="running" meta="Claude · working" {...row("chat2")} />
        </li>
        <li>
          <ListRow icon={<PullRequestIcon />} title="move the score projection to domain" status="idle" meta="widgets#99 · approved" {...row("w99")} />
        </li>
      </ul>
      <h3 class="gallery-group gallery-group-foot">Background agents</h3>
      <ul class="gallery-rows">
        <li>
          <ListRow icon={<BotIcon />} title="Whooper" count="4 queued" status="success" meta="running · next poll 3:45 PM" {...row("whooper")} />
        </li>
      </ul>
    </aside>
  );
}

function DraftThread(props: { annotation: () => DiffLineAnnotation<DraftComment> }) {
  const [included, setIncluded] = createSignal(true);
  const [editing, setEditing] = createSignal(false);
  // The draft starts from the comment's text and then keeps local edits.
  const [body, setBody] = createSignal(untrack(() => props.annotation().metadata.body));
  return (
    <section class="gallery-thread" aria-label={`Draft comment on line ${props.annotation().lineNumber}`}>
      <p class="gallery-thread-meta">
        Whooper · {props.annotation().metadata.label} · score_test.go:{props.annotation().lineNumber} · draft
      </p>
      <Show when={editing()} fallback={<p class="gallery-thread-body">{body()}</p>}>
        <textarea class="gallery-thread-editor" aria-label="Comment" rows={3} value={body()} onInput={(event) => setBody(event.currentTarget.value)} />
      </Show>
      <div class="gallery-thread-actions">
        <Checkbox label="Include in review" checked={included()} onChange={(event) => setIncluded(event.currentTarget.checked)} />
        <Button onClick={() => setEditing((on) => !on)}>{editing() ? "Done" : "Edit"}</Button>
        <Button>Ask about this</Button>
        <Button>Discard</Button>
      </div>
    </section>
  );
}

const statuses: { status: Status; text: string }[] = [
  { status: "attention", text: "Needs you" },
  { status: "running", text: "Running" },
  { status: "success", text: "Running fine" },
  { status: "error", text: "Failed" },
  { status: "idle", text: "Done" },
];

function Gallery() {
  const [layout, setLayout] = createSignal<"split" | "unified">("split");
  const [dialogOpen, setDialogOpen] = createSignal(params.has("dialog"));
  const [lastAction, setLastAction] = createSignal("none yet");
  const [interval, setInterval] = createSignal("15");
  const [pollLimit, setPollLimit] = createSignal("3");
  const annotations: DiffLineAnnotation<DraftComment>[] = drafts.map((draft) => ({ side: "additions", lineNumber: draft.line, metadata: draft }));
  return (
    <div class="gallery">
      <Sidebar />
      <main class="gallery-main">
        <header class="gallery-header">
          <div>
            <h1>@prompting-time/ui</h1>
            <p>Sigil's default preset with Katagami Sans and Mono; every component reads the theme's variables.</p>
          </div>
          <div class="gallery-toolbar">
            <IconButton label="Open on GitHub">
              <ExternalIcon />
            </IconButton>
            <IconButton label="Show inspector">
              <PanelRightIcon />
            </IconButton>
            <Menu
              label="More actions"
              icon={<EllipsisIcon />}
              items={[
                { value: "rerun", label: "Re-run review" },
                { value: "skip", label: "Skip PR" },
                { value: "logs", label: "Open logs", disabled: true },
              ]}
              onSelect={(value) => setLastAction(value)}
            />
          </div>
        </header>
        <p class="gallery-note">Last menu action: {lastAction()}</p>

        <section class="gallery-section" aria-labelledby="buttons-heading">
          <h2 id="buttons-heading">Buttons</h2>
          <div class="gallery-row">
            <Button variant="primary" onClick={() => setDialogOpen(true)}>
              Submit review
            </Button>
            <Button>Retry now</Button>
            <Button>Poll now</Button>
            <Button disabled>Save</Button>
            <IconButton label="Search">
              <SearchIcon />
            </IconButton>
          </div>
        </section>

        <section class="gallery-section" aria-labelledby="form-heading">
          <h2 id="form-heading">Form controls</h2>
          <div class="gallery-form">
            <NumberField label="Poll every" unit="minutes" min={5} max={120} hint="5 to 120" value={interval()} onInput={(event) => setInterval(event.currentTarget.value)} />
            <NumberField
              label="Stop a poll after"
              unit="minutes"
              value={pollLimit()}
              onInput={(event) => setPollLimit(event.currentTarget.value)}
              error="Must be at least 5: a poll can take 2–3 minutes."
            />
            <Select
              label="Review agent"
              options={[
                { value: "devin", label: "Devin" },
                { value: "claude", label: "Claude" },
                { value: "codex", label: "Codex" },
              ]}
            />
            <TextField label="Review skills" value="multi-angle-review, code-shape-and-runtime-review" />
            <Checkbox label="When someone tags you in Slack or requests your review on GitHub" checked />
            <Checkbox label="Untagged team asks" />
          </div>
        </section>

        <GalleryTabs />

        <section class="gallery-section" aria-labelledby="status-heading">
          <h2 id="status-heading">Status dots</h2>
          <ul class="gallery-statuses">
            <For each={statuses}>
              {(entry) => (
                <li>
                  <StatusDot status={entry.status} />
                  {entry.text}
                </li>
              )}
            </For>
          </ul>
        </section>

        <section class="gallery-section" aria-labelledby="diff-heading">
          <div class="gallery-diff-header">
            <h2 id="diff-heading">Diff</h2>
            <div class="gallery-segmented" role="group" aria-label="Diff layout">
              <Button aria-pressed={layout() === "split" ? "true" : "false"} onClick={() => setLayout("split")}>
                Split
              </Button>
              <Button aria-pressed={layout() === "unified" ? "true" : "false"} onClick={() => setLayout("unified")}>
                Unified
              </Button>
            </div>
          </div>
          <Diff
            oldFile={oldFile}
            newFile={newFile}
            layout={layout()}
            annotations={annotations}
            annotationKey={(annotation) => annotation.metadata.id}
            renderAnnotation={(annotation) => <DraftThread annotation={annotation} />}
          />
        </section>
      </main>

      <ConfirmDialog
        open={dialogOpen()}
        title="Submit this review to GitHub?"
        confirmLabel="Submit review"
        onConfirm={() => {
          setLastAction("submitted");
          setDialogOpen(false);
        }}
        onCancel={() => setDialogOpen(false)}
      >
        <p>2 comments and an approval with the message "Pre-approving to unblock" post to acme/widgets#101 as one review.</p>
      </ConfirmDialog>
    </div>
  );
}

render(() => <Gallery />, document.getElementById("root")!);

/** Tabs over one panel, and a menu that opens from a text button, as a review's versions do. */
function GalleryTabs() {
  const [tab, setTab] = createSignal("runs");
  const [version, setVersion] = createSignal("2");
  return (
    <section class="gallery-section" aria-labelledby="tabs-heading">
      <h2 id="tabs-heading">Tabs and a text menu</h2>
      <Tabs
        label="Whooper"
        tabs={[{ value: "runs", label: "Runs" }, { value: "logs", label: "Logs" }, { value: "settings", label: "Settings" }]}
        value={tab()}
        onChange={setTab}
      >
        <p class="gallery-note">The {tab()} panel.</p>
      </Tabs>
      <Menu
        label="Reviews of this pull request"
        text={`Review ${version()} · ${version() === "2" ? "dc0c0bf · 3:12 PM" : "a1b2c3d · Nov 24"}`}
        items={[
          { value: "2", label: "Review 2 · dc0c0bf · 3:12 PM", type: "radio", checked: version() === "2" },
          { value: "1", label: "Review 1 · a1b2c3d · Nov 24", type: "radio", checked: version() === "1" },
        ]}
        onSelect={setVersion}
      />
    </section>
  );
}
