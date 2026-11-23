/**
 * Знак NEXUS: изометрический куб с внутренними рёбрами.
 * Используется в шапке, на экране входа и в боковой панели.
 */
export function LogoMark({ size = 21 }: { size?: number }) {
  return (
    <span className="brand__mark">
      <svg
        viewBox="0 0 32 32"
        width={size}
        height={size}
        fill="none"
        aria-hidden="true"
        focusable="false"
      >
        <path d="M16 4.5 27 10.5v11L16 27.5 5 21.5v-11Z" stroke="currentColor" strokeWidth="1.6" />
        <path
          d="m16 16 11-5.5M16 16 5 10.5M16 16v11.5"
          stroke="currentColor"
          strokeWidth="1.1"
          opacity="0.65"
        />
        <path
          d="M10.5 13.2v5.6L16 22l5.5-3.2v-5.6"
          stroke="currentColor"
          strokeWidth="1.1"
          opacity="0.4"
        />
      </svg>
    </span>
  );
}
