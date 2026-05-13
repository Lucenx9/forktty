import { describe, expect, it } from "vitest";
import { buildHighlightParts, findSearchMatch } from "./search-match";

describe("findSearchMatch", () => {
  it("returns an empty match for blank queries", () => {
    expect(findSearchMatch("Branch Picker", "   ")).toEqual({
      score: 0,
      matchedIndices: [],
    });
  });

  it("prefers direct contiguous matches", () => {
    expect(findSearchMatch("Branch Picker", "Pick")).toEqual({
      score: 0,
      matchedIndices: [7, 8, 9, 10],
    });
  });

  it("supports fuzzy matches across whitespace and casing", () => {
    expect(findSearchMatch("New Branch From HEAD", "nbfh")).toMatchObject({
      matchedIndices: [0, 4, 11, 16],
    });
  });

  it("returns null when the query cannot be matched", () => {
    expect(findSearchMatch("Workspace", "zzz")).toBeNull();
  });
});

describe("buildHighlightParts", () => {
  it("returns the original text when there is no match", () => {
    expect(buildHighlightParts("ForkTTY", [])).toEqual([
      { text: "ForkTTY", matched: false },
    ]);
  });

  it("splits contiguous and non-contiguous matches into stable segments", () => {
    expect(buildHighlightParts("Branch", [0, 1, 4, 5])).toEqual([
      { text: "Br", matched: true },
      { text: "an", matched: false },
      { text: "ch", matched: true },
    ]);
  });
});
