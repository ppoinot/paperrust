export function BrandMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true">
      <path
        d="M5.2 2.8h9.7L18.8 6.7V21.2H5.2Z"
        fill="currentColor"
        opacity="0.14"
      />
      <path
        d="M5.2 2.8h9.7L18.8 6.7V21.2H5.2Z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.55"
        strokeLinejoin="round"
      />
      <path d="M14.9 2.8v3.9h3.9" fill="currentColor" opacity="0.28" />
      <text
        x="12"
        y="15.8"
        textAnchor="middle"
        fill="#c45c32"
        fontFamily="ui-sans-serif, system-ui, sans-serif"
        fontSize="7.4"
        fontWeight="700"
        letterSpacing="-0.04em"
      >
        Pr
      </text>
    </svg>
  );
}
