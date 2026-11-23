import { useId } from 'react';

export interface SeriesPoint {
  at: string;
  value: number;
}

/**
 * Линейный график на SVG. Строится только из реальных точек истории;
 * при отсутствии данных показывает пустое состояние, а не фиктивные значения.
 */
export function Sparkline({
  points,
  height = 120,
  color = 'var(--accent)',
  format = (value: number) => String(Math.round(value)),
  label,
  max,
}: {
  points: SeriesPoint[];
  height?: number;
  color?: string;
  format?: (value: number) => string;
  label?: string;
  max?: number;
}) {
  const gradientId = useId();
  const width = 640;
  const padding = { top: 10, right: 8, bottom: 16, left: 8 };

  if (points.length === 0) {
    return (
      <div className="chart chart--empty" style={{ height }}>
        Нет данных за выбранный интервал
      </div>
    );
  }

  const values = points.map((point) => point.value);
  const rawMax = max ?? Math.max(...values, 0.0001);
  const top = rawMax <= 0 ? 1 : rawMax * 1.1;
  const innerWidth = width - padding.left - padding.right;
  const innerHeight = height - padding.top - padding.bottom;

  const coordinates = points.map((point, index) => {
    const x =
      points.length === 1
        ? padding.left + innerWidth / 2
        : padding.left + (index / (points.length - 1)) * innerWidth;
    const ratio = Math.min(1, Math.max(0, point.value / top));
    const y = padding.top + innerHeight - ratio * innerHeight;
    return { x, y, point };
  });

  const line = coordinates.map((c) => `${c.x.toFixed(1)},${c.y.toFixed(1)}`).join(' ');
  const area = `${padding.left},${padding.top + innerHeight} ${line} ${(padding.left + innerWidth).toFixed(1)},${padding.top + innerHeight}`;
  const last = coordinates[coordinates.length - 1];

  return (
    <div className="chart">
      <svg
        viewBox={`0 0 ${width} ${height}`}
        preserveAspectRatio="none"
        role="img"
        aria-label={label ?? 'График'}
      >
        <defs>
          <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={color} stopOpacity="0.32" />
            <stop offset="100%" stopColor={color} stopOpacity="0" />
          </linearGradient>
        </defs>
        <polygon points={area} fill={`url(#${gradientId})`} />
        <polyline
          points={line}
          fill="none"
          stroke={color}
          strokeWidth="2"
          strokeLinejoin="round"
          strokeLinecap="round"
          vectorEffect="non-scaling-stroke"
        />
        <circle cx={last.x} cy={last.y} r="3" fill={color} />
      </svg>
      <div className="chart__foot">
        <span className="tiny muted">
          {new Date(points[0].at).toLocaleTimeString('ru-RU', {
            hour: '2-digit',
            minute: '2-digit',
          })}
        </span>
        <span className="tiny">
          {label ? `${label}: ` : ''}
          <strong>{format(values[values.length - 1])}</strong>
        </span>
        <span className="tiny muted">
          {new Date(points[points.length - 1].at).toLocaleTimeString('ru-RU', {
            hour: '2-digit',
            minute: '2-digit',
          })}
        </span>
      </div>
    </div>
  );
}
