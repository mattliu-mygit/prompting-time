import { createMemo, createSignal, For, onSettled, Show, untrack } from "solid-js";
import { Button } from "@prompting-time/ui";
import { listenToViewKeys, ViewHead, ViewLimits } from "./FullView";
import { plural, Series } from "./ReviewTop";
import type { ExampleRow, Examples } from "./shown";
import { viewName } from "./views";

/** Each status in words; "same" and "changed" compare a run at the base with one at the head. */
const STATUS: Record<ExampleRow["status"], string> = { same: "same", different: "changed", flaky: "varies between runs", error: "run failed" };
/** How each row ran, in words. */
const LABEL: Record<ExampleRow["label"], string> = { ran_base_and_head: "Ran at base and head", ran_head_only: "Ran at head only", not_compared: "Not compared" };

/** Whether a row's result changed: it differs, and it ran at base and head, so there were two results to compare. */
const changed = (row: ExampleRow) => row.status === "different" && row.label === "ran_base_and_head";
/** The row's status in words, or null for "same" or "different" on a row that didn't run at both, which has nothing to compare. */
const statusWord = (row: ExampleRow) => (row.label !== "ran_base_and_head" && (row.status === "same" || row.status === "different") ? null : STATUS[row.status]);

/**
 * Example inputs, before and after, in one line under What this PR does: the inputs whose result
 * changed between the base and the head, by function, or that none did, and Open all inputs, which
 * shows every input's table in the walkthrough's place.
 */
export function ExamplesLine(props: { examples: Examples; onOpen(button: HTMLButtonElement): void }) {
  /** The changed inputs by function, in the order the plugin lists the rows. */
  const byFunction = createMemo(() => {
    const inputs = new Map<string, string[]>();
    for (const row of props.examples.rows.filter(changed)) inputs.set(row.function, [...(inputs.get(row.function) ?? []), row.input]);
    return [...inputs].map(([name, list]) => ({ name, inputs: list.map((input) => ({ text: input, code: true })) }));
  });
  const ranBoth = () => props.examples.rows.filter((row) => row.label === "ran_base_and_head").length;
  return (
    <div class="walk-view-line" role="group" aria-label={viewName("inputs")}>
      <p>
        <span class="walk-view-name">{viewName("inputs")}</span>
        <Show
          when={byFunction().length > 0}
          fallback={ranBoth() > 0 ? `No result changed for the ${plural(ranBoth(), "input")} run at base and head.` : "No input ran at both base and head, so none could be compared."}
        >
          <For each={byFunction()}>
            {(entry, index) => (
              <>
                {index() === 0 ? "" : index() === byFunction().length - 1 ? ", and " : ", "}
                <code class="walk-code">{entry.name}</code>
                {index() === 0 ? " gives a different result for " : " for "}
                <Series items={entry.inputs} />
              </>
            )}
          </For>
          . <span class="walk-muted">Ran at base and head.</span>
        </Show>
      </p>
      <Button onClick={(event) => props.onOpen(event.currentTarget)}>Open all inputs</Button>
    </div>
  );
}

/** A value a run gave, or a dash for none, such as the base's for a function new in the pull request. */
function Value(props: { text: string | null }) {
  return (
    <Show when={props.text !== null} fallback={<span class="walk-muted"><span aria-hidden="true">–</span><span class="pt-visually-hidden">none</span></span>}>
      <code class="example-value">{props.text}</code>
    </Show>
  );
}

/**
 * Example inputs, before and after, in full, in the walkthrough's place: a table per function, with
 * each input, what it gave at the base (before) and at the head (after), whether the two are the
 * same, changed (in orange), vary between runs, or a run failed, and how it ran, in words, with the
 * plugin's note; the rows whose result changed come first. Only rows that say how they ran show,
 * so nothing here is predicted. Last, what the runs may miss. j and k step through the inputs;
 * Escape clears the current one, then goes back. Nothing moves on a key.
 */
export function ExamplesView(props: { examples: Examples; onBack(): void }) {
  /** The functions the plugin lists, then any a row names that it doesn't, each with its rows, changed first. */
  const tables = createMemo(() => {
    const names = [...new Set([...props.examples.functions.map((item) => item.name), ...props.examples.rows.map((row) => row.function)])];
    return names.map((name) => ({
      name,
      at: props.examples.functions.find((item) => item.name === name) ?? null,
      rows: props.examples.rows.filter((row) => row.function === name).sort((left, right) => Number(changed(right)) - Number(changed(left))),
    })).filter((table) => table.rows.length > 0);
  });
  /** Every row in reading order, the order j and k step in. */
  const rows = createMemo(() => tables().flatMap((table) => table.rows));
  const [current, setCurrent] = createSignal<ExampleRow | null>(null);
  let heading: HTMLHeadingElement | undefined;
  let column: HTMLDivElement | undefined;

  /** j and k: the next or previous input, around the end, its row brought into view at once. */
  function step(by: 1 | -1) {
    const all = untrack(rows);
    const now = all.indexOf(untrack(current) as ExampleRow);
    const next = now < 0 ? (by > 0 ? 0 : all.length - 1) : (now + by + all.length) % all.length;
    setCurrent(all[next] ?? null);
    column?.querySelectorAll<HTMLElement>(".examples-table tbody tr")[next]?.scrollIntoView?.({ block: "nearest" });
  }

  onSettled(() => {
    heading?.focus();
    return listenToViewKeys({
      step,
      escape() {
        if (untrack(current) !== null) setCurrent(null);
        else untrack(() => props.onBack)();
      },
    });
  });

  return (
    <section class="full-view" aria-labelledby="inputs-heading">
      <ViewHead id="inputs" steps="step through the inputs" headingRef={(element) => { heading = element; }} onBack={() => props.onBack()} />
      <div class="full-view-scroll">
        <div class="full-view-column" ref={(element: HTMLDivElement) => { column = element; }}>
          <For each={tables()}>
            {(table, index) => (
              <section class="examples-function" aria-labelledby={`examples-function-${index()}`}>
                <h4 id={`examples-function-${index()}`}>
                  <code>{table.name}</code>
                  <Show when={table.at}>{(at) => <>{" "}<span class="walk-muted">{at().path}:{at().line}</span></>}</Show>
                </h4>
                <table class="examples-table">
                  <thead>
                    <tr><th scope="col">Input</th><th scope="col">Before</th><th scope="col">After</th><th scope="col">Compared</th><th scope="col">How it ran</th></tr>
                  </thead>
                  <tbody>
                    <For each={table.rows}>
                      {(row) => (
                        <tr aria-current={current() === row ? "true" : undefined}>
                          <td><code class="example-value">{row.input}</code></td>
                          <td><Value text={row.before} /></td>
                          <td><Value text={row.after} /></td>
                          <td>
                            <Show when={statusWord(row)} fallback={<span class="walk-muted"><span aria-hidden="true">–</span><span class="pt-visually-hidden">nothing to compare</span></span>}>
                              {(word) => <span class="example-status" data-changed={changed(row) ? "" : undefined}>{word()}</span>}
                            </Show>
                          </td>
                          <td>
                            {LABEL[row.label]}
                            <Show when={row.note}>{(note) => <span class="walk-muted"> · {note()}</span>}</Show>
                          </td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </table>
              </section>
            )}
          </For>
          <Show when={props.examples.limits.length > 0}>
            <ViewLimits id="inputs" heading="What these runs may miss" limits={props.examples.limits} />
          </Show>
        </div>
      </div>
    </section>
  );
}
