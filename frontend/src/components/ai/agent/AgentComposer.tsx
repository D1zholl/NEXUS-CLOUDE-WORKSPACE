/** Поле ввода Agent-панели: Enter — отправить, Shift+Enter — перенос. */
import { useRef } from 'react';
import {
  Bot,
  Cpu,
  Loader2,
  Paperclip,
  Send,
  Sparkles,
  Square,
  X,
} from 'lucide-react';

import type { AiAttachment, AiSkill } from '../../../services/ai';

interface AgentComposerProps {
  value: string;
  onChange: (value: string) => void;
  onSend: () => void;
  onStop: () => void;
  canSend: boolean;
  streaming: boolean;
  sending: boolean;
  disabledReason: string | null;
  attachments: AiAttachment[];
  onAttach: (file: File) => Promise<void>;
  onRemoveAttachment: (id: string) => void;
  attaching: boolean;
  models: string[];
  model: string;
  onModel: (model: string) => void;
  skills: AiSkill[];
  skillIds: string[];
  onSkills: (ids: string[]) => void;
}

export function AgentComposer({
  value,
  onChange,
  onSend,
  onStop,
  canSend,
  streaming,
  sending,
  disabledReason,
  attachments,
  onAttach,
  onRemoveAttachment,
  attaching,
  models,
  model,
  onModel,
  skills,
  skillIds,
  onSkills,
}: AgentComposerProps) {
  const fileRef = useRef<HTMLInputElement>(null);
  const lines = Math.min(8, Math.max(2, value.split('\n').length));

  return (
    <div className="agent-composer">
      {attachments.length > 0 ? (
        <div className="agent-composer__attachments">
          {attachments.map((attachment) => (
            <span key={attachment.id} className="agent-chip">
              <Paperclip aria-hidden="true" />
              {attachment.name}
              <button
                type="button"
                title="Убрать вложение"
                onClick={() => onRemoveAttachment(attachment.id)}
              >
                <X aria-hidden="true" />
              </button>
            </span>
          ))}
        </div>
      ) : null}
      <textarea
        value={value}
        rows={lines}
        placeholder={
          disabledReason ??
          'Опишите задачу. Например: «Создай React-приложение с авторизацией и админ-панелью»'
        }
        disabled={!canSend}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' && !event.shiftKey) {
            event.preventDefault();
            if (canSend && value.trim() && !sending) onSend();
          }
        }}
        aria-label="Сообщение агенту"
      />
      <div className="agent-composer__bar">
        <div className="agent-composer__left">
          <button
            type="button"
            className="ai-icon-btn"
            title="Прикрепить файл"
            disabled={!canSend || attaching}
            onClick={() => fileRef.current?.click()}
          >
            {attaching ? <Loader2 className="spin" aria-hidden="true" /> : <Paperclip aria-hidden="true" />}
          </button>
          <input
            ref={fileRef}
            type="file"
            hidden
            onChange={(event) => {
              const file = event.target.files?.[0];
              event.target.value = '';
              if (file) void onAttach(file);
            }}
          />
          <label className="agent-model" title="Модель, если Runtime её поддерживает">
            <Cpu aria-hidden="true" />
            {models.length > 0 ? (
              <select value={model} onChange={(event) => onModel(event.target.value)}>
                {models.map((item) => (
                  <option key={item} value={item}>
                    {item}
                  </option>
                ))}
              </select>
            ) : (
              <span>{model || 'модель Runtime'}</span>
            )}
          </label>
          {skills.length > 0 ? (
            <details className="agent-skills">
              <summary title="Назначенные Skills">
                <Sparkles aria-hidden="true" />
                Skills {skillIds.length > 0 ? `(${skillIds.length})` : ''}
              </summary>
              <div className="agent-skills__list">
                {skills.map((skill) => (
                  <label key={skill.id}>
                    <input
                      type="checkbox"
                      checked={skillIds.includes(skill.id)}
                      onChange={(event) =>
                        onSkills(
                          event.target.checked
                            ? [...skillIds, skill.id]
                            : skillIds.filter((id) => id !== skill.id),
                        )
                      }
                    />
                    <span>{skill.name}</span>
                    <span className="muted">v{skill.version}</span>
                  </label>
                ))}
              </div>
            </details>
          ) : null}
        </div>
        <div className="agent-composer__right">
          <span className="agent-composer__hint">Enter — отправить · Shift+Enter — перенос</span>
          {streaming ? (
            <button type="button" className="btn btn--sm btn--danger" onClick={onStop}>
              <Square aria-hidden="true" /> Остановить
            </button>
          ) : (
            <button
              type="button"
              className="btn btn--sm btn--primary"
              disabled={!canSend || !value.trim() || sending}
              onClick={onSend}
            >
              {sending ? (
                <Loader2 className="spin" aria-hidden="true" />
              ) : (
                <Send aria-hidden="true" />
              )}
              Отправить
            </button>
          )}
        </div>
      </div>
      {disabledReason ? <p className="agent-composer__disabled">{disabledReason}</p> : null}
      {!disabledReason && !canSend ? (
        <p className="agent-composer__disabled">
          <Bot aria-hidden="true" /> Отправка недоступна: нет прав или активного Runtime.
        </p>
      ) : null}
    </div>
  );
}
