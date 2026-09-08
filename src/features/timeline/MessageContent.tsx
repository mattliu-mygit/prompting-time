import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import Markdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { createLowlight } from "lowlight";
import rust from "highlight.js/lib/languages/rust";
import go from "highlight.js/lib/languages/go";
import javascript from "highlight.js/lib/languages/javascript";
import typescript from "highlight.js/lib/languages/typescript";
import python from "highlight.js/lib/languages/python";
import bash from "highlight.js/lib/languages/bash";
import json from "highlight.js/lib/languages/json";
import css from "highlight.js/lib/languages/css";
import sql from "highlight.js/lib/languages/sql";
import xml from "highlight.js/lib/languages/xml";

const highlighter = createLowlight({ rust, go, javascript, typescript, python, bash, json, css, sql, xml });
const MAX_HIGHLIGHT_LENGTH = 16_384;
type HighlightNode = ReturnType<typeof highlighter.highlight>["children"][number];

function highlightNode(node: HighlightNode, index: number): ReactNode {
  if (node.type === "text") return node.value;
  if (node.type !== "element") return null;
  return <span key={index} className={Array.isArray(node.properties.className) ? node.properties.className.join(" ") : undefined}>
    {node.children.map((child, childIndex) => highlightNode(child as HighlightNode, childIndex))}
  </span>;
}

export function CopyButton({ content, label }: { content: string; label: string }) {
  const [feedback, setFeedback] = useState<"copied" | "failed" | null>(null);
  const requestGeneration = useRef(0);
  useLayoutEffect(() => {
    setFeedback(null);
    return () => { requestGeneration.current += 1; };
  }, [content]);
  async function copy() {
    const generation = ++requestGeneration.current;
    try {
      await navigator.clipboard.writeText(content);
      if (generation === requestGeneration.current) setFeedback("copied");
    } catch {
      if (generation === requestGeneration.current) setFeedback("failed");
    }
  }
  return <span className="copy-control">
    <button type="button" className="disclosure-link" onClick={() => void copy()}>{label}</button>
    {feedback ? <span role="status">{feedback === "copied" ? "Copied" : "Could not copy. Try again."}</span> : null}
  </span>;
}

function CodeBlock({ code, language }: { code: string; language: string }) {
  const [settledCode, setSettledCode] = useState<string | null>(null);
  // Text appears immediately; only the optional highlighting waits for a quiet moment.
  useEffect(() => {
    if (code.length > MAX_HIGHLIGHT_LENGTH || !highlighter.registered(language)) return;
    const timer = window.setTimeout(() => setSettledCode(code), 100);
    return () => window.clearTimeout(timer);
  }, [code, language]);
  const highlighted = useMemo(() => {
    if (settledCode !== code || code.length > MAX_HIGHLIGHT_LENGTH || !highlighter.registered(language)) return null;
    return highlighter.highlight(language, code).children.map(highlightNode);
  }, [code, language, settledCode]);
  return <div className="code-block">
    <div className="code-toolbar"><span>{language || "Code"}</span><CopyButton content={code} label="Copy code" /></div>
    <pre><code className={language ? `language-${language}` : undefined}>{highlighted ?? code}</code></pre>
  </div>;
}

const components: Components = {
  pre({ node }) {
    const code = node?.children.find((child) => child.type === "element" && child.tagName === "code");
    if (!code || code.type !== "element") return null;
    const classNames = code.properties.className;
    const languageClass = Array.isArray(classNames) ? classNames.find((name) => String(name).startsWith("language-")) : undefined;
    return <CodeBlock code={code.children.map((child) => child.type === "text" ? child.value : "").join("")} language={String(languageClass ?? "").replace(/^language-/, "")} />;
  },
  img({ alt }) { return <span className="image-placeholder">{alt ? `[Image: ${alt}]` : "[Image]"}</span>; },
  a({ href, children }) {
    if (!href) return <span>{children}</span>;
    if (href.startsWith("#")) return <a href={href}>{children}</a>;
    return <a href={href} target="_blank" rel="noreferrer noopener">{children}</a>;
  },
  table({ children }) { return <div className="markdown-table"><table>{children}</table></div>; },
};

function safeUrl(url: string) {
  return /^(?:https?:\/\/|#)/i.test(url) ? url : "";
}

export const MessageContent = memo(function MessageContent({ content }: { content: string }) {
  return <div className="message-markdown"><Markdown remarkPlugins={[remarkGfm]} components={components} skipHtml urlTransform={safeUrl}>{content}</Markdown></div>;
});
