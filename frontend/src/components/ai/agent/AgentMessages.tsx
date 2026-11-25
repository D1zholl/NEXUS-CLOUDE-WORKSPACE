/** История сообщений Agent-задачи с пагинацией и streaming. */
import { useEffect, useRef } from 'react';
import { Bot, Loader2, MessageSquare, Pencil, RefreshCw, User } from 'lucide-react';

import { AiMarkdown } from '../../AiMarkdown';
import { Badge, Button } from '../../ui';
import { formatDateTimeShort } from '../../../lib/format';
import type { AiMessage } from '../../../services/ai';

interface AgentMessagesProps {
  messages: AiMessage[];
  loading: boolean;
  loadingOlder: boolean;
  hasOlder: boolean;
  historical: boolean;
  streaming: boolean;
  canSend: boolean;
  onLoadOlder: () => void;
  onEdit: (message: AiMessage) => void;
  onRegenerate: () => void;
  scrollRef: React.RefObject<HTMLDivElement | null>;
}

function roleLabel(role: AiMessage['role']): string {
  switch (role) {
    case 'user':
      return 'Вы';
    case 'assistant':
      return 'AI Agent';
    case 'tool':
      return 'Инструмент';
    default:
      return role;
  }
}

export function AgentMessages({
  messages,
  loading,
  loadingOlder,
  hasOlder,
  historical,
  streaming,
  canSend,
  onLoadOlder,
  onEdit,
  onRegenerate,
  scrollRef,
}: AgentMessagesProps) {
  const bottomRef = useRef<HTMLDivElement>(null);
  const lastId = messages[messages.length - 1]?.id;
  const lastLength = messages[messages.length - 1]?.content.length ?? 0;

  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const distance = element.scrollHeight - element.scrollTop - element.clientHeight;
    if (distance < 240) {
      element.scrollTop = element.scrollHeight;
    }
  }, [lastId, lastLength, scrollRef]);

  if (loading && messages.length === 0) {
    return (
      <div className="agent-messages agent-messages--empty">
        <Loader2 className="spin" aria-hidden="true" />
        <span>Загрузка истории…</span>
      </div>
    );
  }

  if (messages.length === 0) {
    return (
      <div className="agent-messages agent-messages--empty">
        <MessageSquare aria-hidden="true" />
        <h3>Задача ещё не начата</h3>
        <p className="muted">
          Опишите, что нужно сделать в workspace. Агент прочитает файлы, составит план и
          выполнит изменения через Runtime.
        </p>
        {historical ? (
          <p className="muted">Историческая задача не содержит сообщений.</p>
        ) : null}
      </div>
    );
  }

  return (
    <div className="agent-messages" ref={scrollRef}>
      {hasOlder ? (
        <div className="agent-messages__older">
          <Button size="sm" variant="ghost" loading={loadingOlder} onClick={onLoadOlder}>
            Загрузить более ранние сообщения
          </Button>
        </div>
      ) : null}
      {messages.map((message, index) => {
        const isLastAssistant =
          message.role === 'assistant' &&
          index === messages.length - 1 &&
          message.status !== 'streaming';
        return (
          <article
            key={message.id}
            className={`agent-message agent-message--${message.role}${
              message.status === 'failed' ? ' is-failed' : ''
            }`}
          >
            <header className="agent-message__head">
              <span className="agent-message__avatar">
                {message.role === 'user' ? (
                  <User aria-hidden="true" />
                ) : (
                  <Bot aria-hidden="true" />
                )}
              </span>
              <span className="agent-message__role">{roleLabel(message.role)}</span>
              <span className="agent-message__time">{formatDateTimeShort(message.createdAt)}</span>
              {message.model ? (
                <span className="agent-message__model">{message.model}</span>
              ) : null}
              {message.status === 'streaming' ? (
                <Badge tone="info">генерация</Badge>
              ) : message.status === 'cancelled' ? (
                <Badge tone="warn">остановлено</Badge>
              ) : message.status === 'failed' ? (
                <Badge tone="danger">ошибка</Badge>
              ) : null}
              {message.role === 'user' && canSend && !historical ? (
                <button
                  type="button"
                  className="ai-icon-btn agent-message__edit"
                  title="Редактировать сообщение"
                  onClick={() => onEdit(message)}
                >
                  <Pencil aria-hidden="true" />
                </button>
              ) : null}
            </header>
            <div className="agent-message__body">
              {message.role === 'user' ? (
                <p className="agent-message__text">{message.content}</p>
              ) : message.content ? (
                <AiMarkdown content={message.content} />
              ) : message.status === 'streaming' ? (
                <span className="agent-message__typing">
                  <span />
                  <span />
                  <span />
                </span>
              ) : (
                <p className="muted">Пустой ответ.</p>
              )}
            </div>
            {message.errorMessage ? (
              <p className="agent-message__error">
                {message.errorCode}: {message.errorMessage}
              </p>
            ) : null}
            {isLastAssistant && canSend && !historical ? (
              <div className="agent-message__actions">
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<RefreshCw aria-hidden="true" />}
                  onClick={onRegenerate}
                  disabled={streaming}
                >
                  Повторить ответ
                </Button>
              </div>
            ) : null}
          </article>
        );
      })}
      <div ref={bottomRef} />
    </div>
  );
}
