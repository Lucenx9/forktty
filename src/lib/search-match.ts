export interface SearchMatchResult {
  score: number;
  matchedIndices: number[];
}

function compactTextWithMap(text: string): { compact: string; map: number[] } {
  let compact = "";
  const map: number[] = [];

  for (let i = 0; i < text.length; i += 1) {
    if (/\s/.test(text[i] ?? "")) {
      continue;
    }
    compact += text[i];
    map.push(i);
  }

  return { compact, map };
}

export function findSearchMatch(text: string, query: string): SearchMatchResult | null {
  const trimmedQuery = query.trim().toLowerCase();
  if (!trimmedQuery) {
    return { score: 0, matchedIndices: [] };
  }

  const normalizedText = text.toLowerCase();
  const directIndex = normalizedText.indexOf(trimmedQuery);
  if (directIndex !== -1) {
    return {
      score: Math.max(0, directIndex - 8),
      matchedIndices: Array.from(
        { length: trimmedQuery.length },
        (_, offset) => directIndex + offset,
      ),
    };
  }

  const compactQuery = trimmedQuery.replace(/\s+/g, "");
  if (!compactQuery) {
    return { score: 0, matchedIndices: [] };
  }

  const { compact, map } = compactTextWithMap(normalizedText);
  let cursor = 0;
  let score = 6;
  const matchedIndices: number[] = [];

  for (const char of compactQuery) {
    const index = compact.indexOf(char, cursor);
    if (index === -1) {
      return null;
    }
    matchedIndices.push(map[index]!);
    score += index === cursor ? 0 : index - cursor + 1;
    cursor = index + 1;
  }

  return { score, matchedIndices };
}

export function buildHighlightParts(
  text: string,
  matchedIndices: number[],
): Array<{ text: string; matched: boolean }> {
  if (matchedIndices.length === 0) {
    return [{ text, matched: false }];
  }

  const matchedSet = new Set(matchedIndices);
  const parts: Array<{ text: string; matched: boolean }> = [];
  let buffer = "";
  let currentMatch = matchedSet.has(0);

  for (let index = 0; index < text.length; index += 1) {
    const isMatch = matchedSet.has(index);
    const char = text[index] ?? "";

    if (index === 0) {
      currentMatch = isMatch;
      buffer = char;
      continue;
    }

    if (isMatch === currentMatch) {
      buffer += char;
      continue;
    }

    parts.push({ text: buffer, matched: currentMatch });
    currentMatch = isMatch;
    buffer = char;
  }

  if (buffer) {
    parts.push({ text: buffer, matched: currentMatch });
  }

  return parts;
}
