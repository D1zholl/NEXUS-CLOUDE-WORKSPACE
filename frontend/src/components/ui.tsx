import { useEffect, useId, useRef } from 'react';
import { createPortal } from 'react-dom';
import type { ButtonHTMLAttributes, CSSProperties, ReactNode } from 'react';
import { AlertTriangle, CheckCircle2, Eye, Info, X, XCircle } from 'lucide-react';

import type { ToastType } from '../stores/ui';
import { useUi } from '../stores/ui';
import { usePresence } from '../lib/usePresence';

type ButtonVariant = 'default' | 'primary' | 'danger' | 'ghost';

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: 'md' | 'sm';
  iconOnly?: boolean;
  block?: boolean;
  loading?: boolean;
  icon?: ReactNode;
}

export function Button({
  variant = 'default',
  size = 'md',
  iconOnly = false,
  block = false,
  loading = false,
  icon,
  className,
  children,
  disabled,
  type = 'button',
  ...rest
}: ButtonProps) {
  const classes = [
    'btn',
    variant !== 'default' ? `btn--${variant}` : '',
    size === 'sm' ? 'btn--sm' : '',
    iconOnly ? 'btn--icon' : '',
    block ? 'btn--block' : '',
    className ?? '',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <button type={type} className={classes} disabled={disabled || loading} {...rest}>
      {loading ? <span className="btn__spinner" aria-hidden="true" /> : icon}
      {children}
    </button>
  );
}

interface FieldProps {
  label?: ReactNode;
  hint?: ReactNode;
  error?: ReactNode;
  htmlFor?: string;
  full?: boolean;
  children: ReactNode;
}

export function Field({ label, hint, error, htmlFor, full, children }: FieldProps) {
  return (
    <div className={`field${full ? ' field--full' : ''}`}>
      {label ? (
        <label className="field__label" htmlFor={htmlFor}>
          {label}
        </label>
      ) : null}
      {children}
      {error ? <span className="field__error">{error}</span> : null}
      {!error && hint ? <span className="field__hint">{hint}</span> : null}
    </div>
  );
}

/** Поле с кнопкой показа пароля. Кнопка переключает тип у основного input. */
export function PasswordField({
  value,
  onChange,
  autoComplete = 'current-password',
  placeholder,
  id,
  disabled,
}: {
  value: string;
  onChange: (value: string) => void;
  autoComplete?: string;
  placeholder?: string;
  id?: string;
  disabled?: boolean;
}) {
  const localId = useId();
  const inputId = id ?? localId;
  const inputRef = useRef<HTMLInputElement>(null);

  const toggle = (): void => {
    if (!inputRef.current) return;
    inputRef.current.type = inputRef.current.type === 'password' ? 'text' : 'password';
  };

  return (
    <div className="input-wrap">
      <input
        ref={inputRef}
        id={inputId}
        className="input"
        type="password"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        autoComplete={autoComplete}
        placeholder={placeholder}
        disabled={disabled}
        required
      />
      <button
        type="button"
        className="input-wrap__action"
        onClick={toggle}
        aria-label="Показать или скрыть пароль"
        title="Показать или скрыть пароль"
      >
        <Eye aria-hidden="true" />
      </button>
    </div>
  );
}

interface ModalProps {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  description?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  wide?: boolean;
}

const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[contenteditable="true"]',
  '[tabindex]:not([tabindex="-1"])',
].join(', ');

function focusableElements(container: HTMLElement | null): HTMLElement[] {
  if (!container) return [];
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (element) =>
      !element.hasAttribute('disabled') &&
      element.getAttribute('aria-hidden') !== 'true' &&
      element.tabIndex !== -1 &&
      !element.hidden,
  );
}

export function Modal({ open, onClose, title, description, children, footer, wide }: ModalProps) {
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  // Окно остаётся в DOM короткое время после `open=false`, чтобы проиграть
  // exit-анимацию. Фокус, блокировка прокрутки и клавиатура отключаются сразу.
  const { mounted, closing } = usePresence(open, 180);
  // Callback хранится в ref: инлайн-стрелка родителя меняет identity на каждом
  // keystroke, но это не должно перезапускать эффект открытия и переводить
  // фокус на крестик. Эффект зависит только от `open`.
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    if (!open) return;
    const dialog = dialogRef.current;
    const previouslyFocused =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';

    // Начальный фокус — первое редактируемое поле. Крестик и кнопки футера
    // намеренно не выбираются: иначе пользователь начинает ввод не в поле.
    const initial =
      dialog?.querySelector<HTMLElement>('[data-modal-autofocus]') ??
      dialog?.querySelector<HTMLElement>(
        '.modal__body input:not([disabled]):not([type="hidden"]), .modal__body textarea:not([disabled]), .modal__body select:not([disabled]), .modal__body [contenteditable="true"]',
      ) ??
      dialog?.querySelector<HTMLElement>('.modal__title') ??
      dialog;
    initial?.focus({ preventScroll: true });

    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onCloseRef.current();
        return;
      }
      if (event.key !== 'Tab') return;
      const focusable = focusableElements(dialog);
      if (focusable.length === 0) {
        event.preventDefault();
        dialog?.focus({ preventScroll: true });
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const active = document.activeElement;
      const inside = active instanceof Node && dialog?.contains(active);
      if (event.shiftKey) {
        if (active === first || !inside) {
          event.preventDefault();
          last.focus();
        }
      } else if (active === last || !inside) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('keydown', onKeyDown);
      document.body.style.overflow = previousOverflow;
      // Возврат фокуса на триггер, если он ещё существует в документе.
      if (previouslyFocused?.isConnected) {
        previouslyFocused.focus({ preventScroll: true });
      }
    };
  }, [open]);

  if (!mounted) return null;

  return createPortal(
    <div
      className={`modal-backdrop${closing ? ' modal-backdrop--closing' : ''}`}
      aria-hidden={closing || undefined}
      inert={closing || undefined}
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className={`modal${wide ? ' modal--wide' : ''}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
      >
        <div className="modal__head">
          <div>
            <div className="modal__title" id={titleId} tabIndex={-1}>
              {title}
            </div>
            {description ? <div className="modal__desc">{description}</div> : null}
          </div>
          <Button variant="ghost" size="sm" iconOnly onClick={onClose} aria-label="Закрыть">
            <X aria-hidden="true" />
          </Button>
        </div>
        <div className="modal__body">{children}</div>
        {footer ? <div className="modal__foot">{footer}</div> : null}
      </div>
    </div>,
    document.body,
  );
}

const TOAST_ICONS: Record<ToastType, ReactNode> = {
  success: <CheckCircle2 aria-hidden="true" />,
  error: <XCircle aria-hidden="true" />,
  warning: <AlertTriangle aria-hidden="true" />,
  info: <Info aria-hidden="true" />,
};

export function Toaster() {
  const toasts = useUi((state) => state.toasts);
  const dismiss = useUi((state) => state.dismissToast);

  if (toasts.length === 0) return null;

  return (
    <div className="toasts" role="status" aria-live="polite">
      {toasts.map((toast) => (
        <button
          key={toast.id}
          type="button"
          className={`toast toast--${toast.type}${toast.leaving ? ' toast--out' : ''}`}
          onClick={() => dismiss(toast.id)}
          title="Скрыть уведомление"
        >
          {TOAST_ICONS[toast.type]}
          <span>{toast.message}</span>
        </button>
      ))}
    </div>
  );
}

export function Badge({
  tone = 'default',
  plain = false,
  title,
  children,
}: {
  tone?: 'default' | 'ok' | 'warn' | 'danger' | 'info';
  plain?: boolean;
  title?: string;
  children: ReactNode;
}) {
  const classes = ['badge', tone !== 'default' ? `badge--${tone}` : '', plain ? 'badge--plain' : '']
    .filter(Boolean)
    .join(' ');
  return (
    <span className={classes} title={title}>
      {children}
    </span>
  );
}

export function Spinner({ large = false }: { large?: boolean }) {
  return <span className={`spinner${large ? ' spinner--lg' : ''}`} aria-label="Загрузка" />;
}

export function LoadingScreen({ label = 'Загрузка пространства' }: { label?: string }) {
  return (
    <div className="loading-screen">
      <div className="loading-screen__inner">
        <div className="loading-cube" aria-hidden="true">
          <div className="loading-cube__inner">
            <div className="loading-cube__face" />
            <div className="loading-cube__face" />
            <div className="loading-cube__face" />
            <div className="loading-cube__face" />
            <div className="loading-cube__face" />
            <div className="loading-cube__face" />
          </div>
        </div>
        <span>{label}</span>
      </div>
    </div>
  );
}

export function EmptyState({
  icon,
  title,
  children,
  action,
}: {
  icon?: ReactNode;
  title: ReactNode;
  children?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="empty">
      {icon ? <div className="empty__icon">{icon}</div> : null}
      <h3>{title}</h3>
      {children ? <p>{children}</p> : null}
      {action ? <div style={{ marginTop: 8 }}>{action}</div> : null}
    </div>
  );
}

export function Alert({
  tone = 'danger',
  icon,
  children,
}: {
  tone?: 'danger' | 'warn' | 'info';
  icon?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div
      className={`alert${tone !== 'danger' ? ` alert--${tone}` : ''}`}
      role={tone === 'danger' ? 'alert' : undefined}
    >
      {icon ?? <AlertTriangle aria-hidden="true" />}
      <div>{children}</div>
    </div>
  );
}

export function Switch({
  checked,
  onChange,
  label,
  disabled = false,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  label?: string;
  disabled?: boolean;
}) {
  return (
    <div className="toggle-row">
      {label ? <label>{label}</label> : null}
      <button
        type="button"
        role="switch"
        className="switch"
        aria-checked={checked}
        aria-label={label}
        onClick={() => onChange(!checked)}
        disabled={disabled}
      />
    </div>
  );
}

export function RangeControl({
  label,
  value,
  min,
  max,
  step,
  onChange,
  format,
  disabled = false,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (value: number) => void;
  format?: (value: number) => string;
  disabled?: boolean;
}) {
  const id = useId();
  const percent = max === min ? 0 : ((value - min) / (max - min)) * 100;
  return (
    <div className="range">
      <div className="range__head">
        <label htmlFor={id}>{label}</label>
        <output>{format ? format(value) : String(value)}</output>
      </div>
      <input
        id={id}
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        style={{ '--fill': `${percent}%` } as CSSProperties}
        onChange={(event) => onChange(Number(event.target.value))}
      />
    </div>
  );
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
}: {
  value: T;
  options: Array<{ value: T; label: string }>;
  onChange: (value: T) => void;
  label?: string;
}) {
  return (
    <div className="segmented" role="group" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          aria-pressed={value === option.value}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

export function Avatar({ name, small = false }: { name: string; small?: boolean }) {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  const letters =
    parts.length === 0
      ? '?'
      : parts.length === 1
        ? parts[0].slice(0, 2).toUpperCase()
        : `${parts[0][0]}${parts[1][0]}`.toUpperCase();
  return <span className={`avatar${small ? ' avatar--sm' : ''}`}>{letters}</span>;
}

export function PageHead({
  eyebrow,
  title,
  description,
  actions,
}: {
  eyebrow?: ReactNode;
  title: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className="page-head">
      <div>
        {eyebrow ? <div className="eyebrow">{eyebrow}</div> : null}
        <h1 className="page-head__title">{title}</h1>
        {description ? <div className="page-head__desc">{description}</div> : null}
      </div>
      {actions ? <div className="page-head__actions">{actions}</div> : null}
    </div>
  );
}

export function SectionTitle({
  title,
  meta,
  actions,
}: {
  title: ReactNode;
  meta?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className="section-title">
      <h2>{title}</h2>
      {actions ?? (meta ? <span>{meta}</span> : null)}
    </div>
  );
}

export function StatCard({
  label,
  icon,
  value,
  unit,
  sub,
  progress,
  tone = 'default',
}: {
  label: string;
  icon?: ReactNode;
  value: ReactNode;
  unit?: ReactNode;
  sub?: ReactNode;
  progress?: number;
  tone?: 'default' | 'warn';
}) {
  return (
    <div className="stat-card">
      <div className="stat-card__top">
        <span>{label}</span>
        {icon}
      </div>
      <div className="stat-card__value">
        {value}
        {unit ? <small>{unit}</small> : null}
      </div>
      {progress !== undefined ? (
        <div className="meter" aria-hidden="true">
          <div
            className={`meter__fill${tone === 'warn' ? ' meter__fill--warn' : ''}`}
            style={{ width: `${Math.min(100, Math.max(0, progress))}%` }}
          />
        </div>
      ) : null}
      {sub ? <div className="stat-card__sub">{sub}</div> : null}
    </div>
  );
}

export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel = 'Подтвердить',
  cancelLabel = 'Отмена',
  busy = false,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: ReactNode;
  description: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <Modal
      open={open}
      onClose={busy ? () => undefined : onCancel}
      title={title}
      description={description}
      footer={
        <>
          <Button onClick={onCancel} disabled={busy}>
            {cancelLabel}
          </Button>
          <Button variant="danger" onClick={onConfirm} loading={busy}>
            {confirmLabel}
          </Button>
        </>
      }
    >
      <div className="confirm-icon">
        <AlertTriangle aria-hidden="true" />
      </div>
    </Modal>
  );
}

export function LoadingInline({ label = 'Загрузка' }: { label?: string }) {
  return (
    <div
      className="row"
      style={{ justifyContent: 'center', padding: 32, color: 'var(--text-secondary)' }}
    >
      <Spinner />
      <span>{label}</span>
    </div>
  );
}
