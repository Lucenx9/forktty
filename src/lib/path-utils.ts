/**
 * Shorten a filesystem path for display.
 * Replaces /home/<user> with ~, then truncates from the left with "..." if needed.
 */
export function truncatePath(path: string, maxLen: number): string {
  if (path.length <= maxLen) return path;
  const home = path.replace(/^\/home\/[^/]+/, "~");
  if (home.length <= maxLen) return home;
  return "..." + home.slice(home.length - maxLen + 3);
}
