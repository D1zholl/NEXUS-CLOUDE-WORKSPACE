import { Fragment, useEffect, useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import { Check, Copy } from 'lucide-react';

const HLJS_CLASS = /^hljs(?:-[a-z0-9-]+)?$/i;

function highlightedNodes(html: string): ReactNode[] {
  const parsed = new DOMParser().parseFromString(
    `<div data-highlight-root>${html}</div>`,
    'text/html',
  );
  const root = parsed.querySelector('[data-highlight-root]');
  if (!root) return [];
  const convert = (node: ChildNode, key: string): ReactNode => {
    if (node.nodeType === Node.TEXT_NODE) return node.nodeValue;
    if (node.nodeType === Node.ELEMENT_NODE) {
      const element = node as Element;
      const children = Array.from(element.childNodes).map((child, index) =>
        convert(child, `${key}-${index}`),
      );
      const classes = (element.getAttribute('class') ?? '').split(/\s+/).filter(Boolean);
      const safeSpan =
        element.tagName === 'SPAN' &&
        classes.length > 0 &&
        classes.every((className) => HLJS_CLASS.test(className));
      if (safeSpan) {
        return (
          <span key={key} className={classes.join(' ')}>
            {children.map((child, index) => (
              <Fragment key={index}>{child}</Fragment>
            ))}
          </span>
        );
      }
      return <Fragment key={key}>{children}</Fragment>;
    }
    return null;
  };
  return Array.from(root.childNodes).map((node, index) => convert(node, `hljs-${index}`));
}

function CodeBlock({ code, language }: { code: string; language: string | null }) {
  const [highlighted, setHighlighted] = useState<ReactNode[] | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let active = true;
    setHighlighted(null);
    if (!language) return;
    import('../lib/highlight')
      .then(({ highlightCode }) => {
        if (!active) return;
        const result = highlightCode(code, language);
        if (result) setHighlighted(highlightedNodes(result.html));
      })
      .catch(() => {
        // Подсветка не критична.
      });
    return () => {
      active = false;
    };
  }, [code, language]);

  const copy = async (): Promise<void> => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      // Буфер обмена может быть недоступен без HTTPS.
    }
  };

  return (
    <div className="ai-code">
      <div className="ai-code__bar">
        <span>{language ?? 'text'}</span>
        <button
          type="button"
          className="ai-icon-btn"
          onClick={() => void copy()}
          title="Копировать"
        >
          {copied ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
        </button>
      </div>
      <pre className="ai-code__body" data-language={language ?? undefined}>
        {highlighted ?? code}
      </pre>
    </div>
  );
}

function inline(text: string, keyPrefix: string): ReactNode[] {
  const result: ReactNode[] = [];
  const pattern = /(`[^`]+`|\*\*[^*]+\*\*)/g;
  let last = 0;
  let match: RegExpExecArray | null;
  let index = 0;
  while ((match = pattern.exec(text))) {
    if (match.index > last) result.push(text.slice(last, match.index));
    const value = match[0];
    if (value.startsWith('`')) {
      result.push(
        <code key={`${keyPrefix}-c-${index}`} className="ai-inline-code">
          {value.slice(1, -1)}
        </code>,
      );
    } else {
      result.push(<strong key={`${keyPrefix}-b-${index}`}>{value.slice(2, -2)}</strong>);
    }
    last = match.index + value.length;
    index += 1;
  }
  if (last < text.length) result.push(text.slice(last));
  return result;
}

interface Block {
  kind: 'text' | 'code' | 'heading' | 'list' | 'quote';
  lines: string[];
  language?: string;
  level?: number;
}

/** Безопасный Markdown: только React-узлы, без HTML и внешних ссылок. */
export function AiMarkdown({ content }: { content: string }) {
  const blocks = useMemo<Block[]>(() => {
    const result: Block[] = [];
    const lines = content.replace(/\r\n/g, '\n').split('\n');
    let index = 0;
    while (index < lines.length) {
      const line = lines[index];
      if (line.startsWith('```')) {
        const language = line.slice(3).trim() || null;
        const body: string[] = [];
        index += 1;
        while (index < lines.length && !lines[index].startsWith('```')) {
          body.push(lines[index]);
          index += 1;
        }
        index += 1;
        result.push({ kind: 'code', lines: body, language: language ?? undefined });
        continue;
      }
      if (/^#{1,4}\s+/.test(line)) {
        const level = line.match(/^#+/)![0].length;
        result.push({
          kind: 'heading',
          lines: [line.replace(/^#{1,4}\s+/, '')],
          level,
        });
        index += 1;
        continue;
      }
      if (/^\s*([-*]|\d+\.)\s+/.test(line)) {
        const items: string[] = [];
        while (index < lines.length && /^\s*([-*]|\d+\.)\s+/.test(lines[index])) {
          items.push(lines[index].replace(/^\s*([-*]|\d+\.)\s+/, ''));
          index += 1;
        }
        result.push({ kind: 'list', lines: items });
        continue;
      }
      if (line.startsWith('> ')) {
        const quoted: string[] = [];
        while (index < lines.length && lines[index].startsWith('> ')) {
          quoted.push(lines[index].slice(2));
          index += 1;
        }
        result.push({ kind: 'quote', lines: quoted });
        continue;
      }
      if (!line.trim()) {
        index += 1;
        continue;
      }
      const paragraph: string[] = [line];
      index += 1;
      while (
        index < lines.length &&
        lines[index].trim() &&
        !lines[index].startsWith('```') &&
        !/^#{1,4}\s+/.test(lines[index]) &&
        !/^\s*([-*]|\d+\.)\s+/.test(lines[index]) &&
        !lines[index].startsWith('> ')
      ) {
        paragraph.push(lines[index]);
        index += 1;
      }
      result.push({ kind: 'text', lines: [paragraph.join('\n')] });
    }
    return result;
  }, [content]);

  return (
    <div className="ai-markdown">
      {blocks.map((block, index) => {
        if (block.kind === 'code') {
          return (
            <CodeBlock
              key={index}
              code={block.lines.join('\n')}
              language={block.language ?? null}
            />
          );
        }
        if (block.kind === 'heading') {
          const level = block.level ?? 2;
          const text = inline(block.lines[0], `h-${index}`);
          if (level <= 2) return <h3 key={index}>{text}</h3>;
          return <h4 key={index}>{text}</h4>;
        }
        if (block.kind === 'list') {
          return (
            <ul key={index}>
              {block.lines.map((item, itemIndex) => (
                <li key={itemIndex}>{inline(item, `l-${index}-${itemIndex}`)}</li>
              ))}
            </ul>
          );
        }
        if (block.kind === 'quote') {
          return <blockquote key={index}>{inline(block.lines.join(' '), `q-${index}`)}</blockquote>;
        }
        return (
          <p key={index} className="ai-markdown__paragraph">
            {inline(block.lines[0], `p-${index}`)}
          </p>
        );
      })}
    </div>
  );
}
