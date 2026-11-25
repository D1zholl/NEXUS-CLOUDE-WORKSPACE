/**
 * Безопасная подсветка кода: highlight.js возвращает HTML, который
 * разбирается в React-узлы. Вставлять HTML через dangerouslySetInnerHTML
 * нельзя — даже доверенный файл может содержать опасную разметку.
 */
import { Fragment, useEffect, useMemo, useState } from 'react';
import type { ReactNode } from 'react';

const HLJS_CLASS = /^hljs(?:-[a-z0-9-]+)?$/i;

function toNodes(html: string): ReactNode[] {
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
      const safe =
        element.tagName === 'SPAN' &&
        classes.length > 0 &&
        classes.every((name) => HLJS_CLASS.test(name));
      if (safe) {
        return (
          <span key={key} className={classes.join(' ')}>
            {children}
          </span>
        );
      }
      return <Fragment key={key}>{children}</Fragment>;
    }
    return null;
  };
  return Array.from(root.childNodes).map((node, index) => convert(node, `hljs-${index}`));
}

export function useHighlightedCode(
  code: string,
  language: string | null,
  limit = 200_000,
): ReactNode[] | null {
  const [nodes, setNodes] = useState<ReactNode[] | null>(null);
  const eligible = Boolean(language) && code.length <= limit;

  useEffect(() => {
    if (!eligible) {
      setNodes(null);
      return;
    }
    let active = true;
    const timer = window.setTimeout(() => {
      import('../../../lib/highlight')
        .then(({ highlightCode }) => {
          if (!active) return;
          const result = highlightCode(code, language!);
          setNodes(result ? toNodes(result.html) : null);
        })
        .catch(() => {
          if (active) setNodes(null);
        });
    }, 140);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [code, language, eligible]);

  return nodes;
}

export function HighlightedCode({
  code,
  language,
  className,
}: {
  code: string;
  language: string | null;
  className?: string;
}) {
  const nodes = useHighlightedCode(code, language);
  return <code className={className}>{nodes ?? code}</code>;
}

/** Количество строк без создания массива на каждый рендер. */
export function useLineCount(content: string): number {
  return useMemo(() => {
    if (content.length === 0) return 1;
    let count = 1;
    let index = content.indexOf('\n');
    while (index !== -1) {
      count += 1;
      index = content.indexOf('\n', index + 1);
    }
    return count;
  }, [content]);
}
