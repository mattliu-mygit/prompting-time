import type { Element, ElementContent, Properties, RootContent } from "hast";
import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import { Dynamic, type JSX } from "@solidjs/web";
import { unified } from "unified";
import { CopyButton } from "./CopyButton";
import { highlightCode, type CodeToken } from "./highlight";

// remark-rehype drops raw HTML, so HTML in a message never reaches the page.
const processor = unified().use(remarkParse).use(remarkGfm).use(remarkRehype);
const MAX_HIGHLIGHT_LENGTH = 16_384;
const tableParts = new Set(["table", "thead", "tbody", "tfoot", "tr"]);

export interface MarkdownProps {
  /** Markdown source, such as a model's reply. It is untrusted: it cannot run code, embed pages, or load images. */
  content: string;
}

/**
 * Renders untrusted Markdown with GitHub's extensions. HTTP(S) links open in a new window; fragment
 * links stay on the page; other links are plain text. Images show as their alt text. Code blocks
 * can be copied and are highlighted once their text stops changing for a moment.
 */
export function Markdown(props: MarkdownProps) {
  const blocks = createMemo(() => processor.runSync(processor.parse(props.content)).children);
  // Rows are matched by position, so a code block above a streaming paragraph keeps its colors.
  return (
    <div class="pt-markdown">
      <For each={blocks()} keyed={false}>
        {(block) => <Block node={block()} />}
      </For>
    </div>
  );
}

function Block(props: { node: RootContent }) {
  const code = createMemo(() => (props.node.type === "element" ? codeOf(props.node) : null));
  return (
    <Show when={code()} fallback={<>{props.node.type === "doctype" ? null : renderNode(props.node)}</>}>
      {(found) => <CodeBlock code={found().code} language={found().language} />}
    </Show>
  );
}

function renderNode(node: ElementContent): JSX.Element {
  if (node.type === "text") return node.value;
  if (node.type !== "element") return null;
  // Line breaks between table rows and cells are not content, and a table cannot hold text there.
  const children = () =>
    (tableParts.has(node.tagName) ? node.children.filter((child) => child.type !== "text" || child.value.trim() !== "") : node.children).map(renderNode);
  switch (node.tagName) {
    case "pre": {
      const code = codeOf(node);
      return code ? <CodeBlock code={code.code} language={code.language} /> : <pre>{children()}</pre>;
    }
    case "img":
      return <span class="pt-markdown-image">{node.properties.alt ? `[Image: ${String(node.properties.alt)}]` : "[Image]"}</span>;
    case "a": {
      const href = typeof node.properties.href === "string" ? node.properties.href : "";
      if (href.startsWith("#")) return <a {...attributes(node.properties)}>{children()}</a>;
      if (/^https?:\/\//i.test(href)) {
        return (
          <a {...attributes(node.properties)} target="_blank" rel="noreferrer noopener">
            {children()}
          </a>
        );
      }
      return <span>{children()}</span>;
    }
    case "table":
      return (
        <div class="pt-markdown-table">
          <table>{children()}</table>
        </div>
      );
    default:
      return (
        <Dynamic component={node.tagName} {...attributes(node.properties)}>
          {children()}
        </Dynamic>
      );
  }
}

/** The code and language of a fenced code block, which remark-rehype draws as pre > code.language-x. */
function codeOf(element: Element): { code: string; language: string } | null {
  if (element.tagName !== "pre") return null;
  const code = element.children.find((child): child is Element => child.type === "element" && child.tagName === "code");
  if (!code) return null;
  const classes = Array.isArray(code.properties.className) ? code.properties.className.map(String) : [];
  const language = classes.find((name) => name.startsWith("language-"))?.slice("language-".length) ?? "";
  return { code: code.children.map((child) => (child.type === "text" ? child.value : "")).join(""), language };
}

/** hast's property names as HTML attributes: className is class, dataFootnoteRef is data-footnote-ref. */
function attributes(properties: Properties): Record<string, string | true> {
  const result: Record<string, string | true> = {};
  for (const [name, value] of Object.entries(properties)) {
    if (value === null || value === undefined || value === false) continue;
    const attribute =
      name === "className"
        ? "class"
        : /^(data|aria)[A-Z]/.test(name)
          ? name.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)
          : name.toLowerCase();
    result[attribute] = value === true ? true : Array.isArray(value) ? value.join(" ") : String(value);
  }
  return result;
}

function CodeBlock(props: { code: string; language: string }) {
  const [highlighted, setHighlighted] = createSignal<{ code: string; language: string; lines: CodeToken[][] } | null>(null);
  // The text shows at once; only the colors wait until the code has not changed for 100 ms.
  // A memo that changes only with the code or language, since an effect reruns on every change it reads.
  const source = createMemo(() => ({ code: props.code, language: props.language }), {
    equals: (previous, next) => previous.code === next.code && previous.language === next.language,
  });
  createEffect(
    source,
    ({ code, language }) => {
      if (!language || code.length > MAX_HIGHLIGHT_LENGTH) return;
      let current = true;
      const timer = setTimeout(() => {
        void highlightCode(code, language).then((lines) => {
          if (current && lines) setHighlighted({ code, language, lines });
        });
      }, 100);
      return () => {
        current = false;
        clearTimeout(timer);
      };
    },
  );
  const lines = () => {
    const current = highlighted();
    return current && current.code === props.code && current.language === props.language ? current.lines : null;
  };
  return (
    <div class="pt-code-block">
      <div class="pt-code-toolbar">
        <span>{props.language || "Code"}</span>
        <CopyButton content={props.code} label="Copy code" />
      </div>
      <pre>
        <code class={props.language ? `language-${props.language}` : undefined}>
          <Show when={lines()} fallback={props.code}>
            {(tokens) => (
              <For each={tokens()}>
                {(line, index) => (
                  <>
                    {index() > 0 ? "\n" : ""}
                    <For each={line}>{(token) => <span style={token.color ? { color: token.color } : undefined}>{token.content}</span>}</For>
                  </>
                )}
              </For>
            )}
          </Show>
        </code>
      </pre>
    </div>
  );
}
