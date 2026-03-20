interface IconProps {
  size?: number;
  className?: string;
}

export function CloseIcon({ size = 12, className }: IconProps) {
  return (
    <svg
      aria-hidden="true"
      className={className}
      width={size}
      height={size}
      viewBox="0 0 12 12"
      fill="none"
    >
      <path
        d="M2.25 2.25 9.75 9.75M9.75 2.25 2.25 9.75"
        stroke="currentColor"
        strokeLinecap="round"
        strokeWidth="1.4"
      />
    </svg>
  );
}

export function ChevronUpIcon({ size = 12, className }: IconProps) {
  return (
    <svg
      aria-hidden="true"
      className={className}
      width={size}
      height={size}
      viewBox="0 0 12 12"
      fill="none"
    >
      <path
        d="M2.25 7.5 6 3.75 9.75 7.5"
        stroke="currentColor"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="1.4"
      />
    </svg>
  );
}

export function ChevronDownIcon({ size = 12, className }: IconProps) {
  return (
    <svg
      aria-hidden="true"
      className={className}
      width={size}
      height={size}
      viewBox="0 0 12 12"
      fill="none"
    >
      <path
        d="M2.25 4.5 6 8.25 9.75 4.5"
        stroke="currentColor"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="1.4"
      />
    </svg>
  );
}

export function MatchCaseIcon({ size = 14, className }: IconProps) {
  return (
    <svg
      aria-hidden="true"
      className={className}
      width={size}
      height={size}
      viewBox="0 0 14 14"
      fill="none"
    >
      <path
        d="M2.5 11 5 3l2.5 8M3.5 8h3"
        stroke="currentColor"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="1.1"
      />
      <path
        d="M10 10.5c0-.83-.56-1.5-1.25-1.5S7.5 9.67 7.5 10.5 8.06 12 8.75 12 10 11.33 10 10.5Zm0 0V9"
        stroke="currentColor"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="1.1"
      />
    </svg>
  );
}

export function MergeIcon({ size = 12, className }: IconProps) {
  return (
    <svg
      aria-hidden="true"
      className={className}
      width={size}
      height={size}
      viewBox="0 0 12 12"
      fill="none"
    >
      <circle cx="3" cy="3" r="1.25" stroke="currentColor" strokeWidth="1.1" />
      <circle cx="9" cy="3" r="1.25" stroke="currentColor" strokeWidth="1.1" />
      <circle cx="6" cy="9" r="1.25" stroke="currentColor" strokeWidth="1.1" />
      <path
        d="M3.95 3.8 5.55 7.6M8.05 3.8 6.45 7.6"
        stroke="currentColor"
        strokeLinecap="round"
        strokeWidth="1.1"
      />
    </svg>
  );
}

export function TrashIcon({ size = 12, className }: IconProps) {
  return (
    <svg
      aria-hidden="true"
      className={className}
      width={size}
      height={size}
      viewBox="0 0 12 12"
      fill="none"
    >
      <path
        d="M2.75 3.25h6.5M4.25 3.25V2h3.5v1.25M4 4.5v4.25M6 4.5v4.25M8 4.5v4.25M3.5 3.25l.4 6.1c.04.4.37.7.77.7h2.66c.4 0 .73-.3.77-.7l.4-6.1"
        stroke="currentColor"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="1.1"
      />
    </svg>
  );
}

export function GripIcon({ size = 12, className }: IconProps) {
  return (
    <svg
      aria-hidden="true"
      className={className}
      width={size}
      height={size}
      viewBox="0 0 12 12"
      fill="none"
    >
      <circle cx="4" cy="3" r="0.75" fill="currentColor" />
      <circle cx="8" cy="3" r="0.75" fill="currentColor" />
      <circle cx="4" cy="6" r="0.75" fill="currentColor" />
      <circle cx="8" cy="6" r="0.75" fill="currentColor" />
      <circle cx="4" cy="9" r="0.75" fill="currentColor" />
      <circle cx="8" cy="9" r="0.75" fill="currentColor" />
    </svg>
  );
}
