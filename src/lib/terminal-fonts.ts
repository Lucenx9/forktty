const GENERIC_FONT_FAMILIES = new Set([
  "serif",
  "sans-serif",
  "monospace",
  "system-ui",
  "ui-monospace",
]);

const TERMINAL_FONT_FALLBACKS = [
  "DejaVu Sans Mono",
  "Liberation Mono",
  "Noto Mono",
  "FreeMono",
  "monospace",
  "Symbols Nerd Font Mono",
];

function stripWrappingQuotes(value: string): string {
  const trimmed = value.trim();
  if (trimmed.length < 2) return trimmed;

  const first = trimmed[0];
  const last = trimmed[trimmed.length - 1];
  if ((first === '"' && last === '"') || (first === "'" && last === "'")) {
    return trimmed.slice(1, -1).trim();
  }

  return trimmed;
}

function splitFontFamilyList(value: string): string[] {
  const families: string[] = [];
  let current = "";
  let quote: '"' | "'" | null = null;

  for (const char of value) {
    if (quote) {
      if (char === quote) {
        quote = null;
      } else {
        current += char;
      }
      continue;
    }

    if (char === '"' || char === "'") {
      quote = char;
      continue;
    }

    if (char === ",") {
      const cleaned = stripWrappingQuotes(current);
      if (cleaned) families.push(cleaned);
      current = "";
      continue;
    }

    current += char;
  }

  const cleaned = stripWrappingQuotes(current);
  if (cleaned) families.push(cleaned);
  return families;
}

function formatFontFamily(name: string): string {
  const normalized = stripWrappingQuotes(name);
  if (!normalized) return "";
  if (GENERIC_FONT_FAMILIES.has(normalized.toLowerCase())) {
    return normalized;
  }
  const escaped = normalized.replaceAll("\\", "\\\\").replaceAll('"', '\\"');
  return `"${escaped}"`;
}

export function buildTerminalFontFamily(
  preferred: string | null | undefined,
): string {
  const families: string[] = [];
  const seen = new Set<string>();
  const deferredGenerics: string[] = [];

  const addFamily = (family: string) => {
    const formatted = formatFontFamily(family);
    if (!formatted) return;

    const key = stripWrappingQuotes(family).toLowerCase();
    if (seen.has(key)) return;
    seen.add(key);
    families.push(formatted);
  };

  if (preferred) {
    for (const family of splitFontFamilyList(preferred)) {
      const normalized = stripWrappingQuotes(family).toLowerCase();
      if (GENERIC_FONT_FAMILIES.has(normalized)) {
        deferredGenerics.push(family);
        continue;
      }
      addFamily(family);
    }
  }

  for (const fallback of TERMINAL_FONT_FALLBACKS) {
    addFamily(fallback);
  }

  for (const family of deferredGenerics) {
    addFamily(family);
  }

  return families.join(", ");
}
