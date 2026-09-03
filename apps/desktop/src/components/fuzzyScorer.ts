/**
 * Pure fuzzy path scorer for the file finder (Cmd-P).
 *
 * Scoring priority:
 * 1. Basename exact match
 * 2. Basename prefix match
 * 3. Basename substring match
 * 4. Path substring match
 * 5. Path segment prefix match (e.g. "crd/fil" matching "crates/repomon-daemon/src/files.rs")
 * 6. Subsequence match with bonuses for boundaries, consecutive characters, and basename hits
 *
 * Stable tie-breaking: higher score first, then shorter path, then alphabetical.
 */

export interface FuzzyMatch {
  path: string;
  score: number;
  indices: number[];
}

function isBoundaryChar(ch: string): boolean {
  return ch === "/" || ch === "_" || ch === "-" || ch === ".";
}

export function scorePath(path: string, query: string): FuzzyMatch | null {
  const trimmed = query.trim();
  if (trimmed.length === 0) {
    return {
      path,
      score: 1000 - Math.min(500, path.length),
      indices: [],
    };
  }

  const slashIdx = path.lastIndexOf("/");
  const basename = slashIdx >= 0 ? path.slice(slashIdx + 1) : path;
  const basenameStart = slashIdx >= 0 ? slashIdx + 1 : 0;

  const qLower = trimmed.toLowerCase();
  const bLower = basename.toLowerCase();
  const pLower = path.toLowerCase();

  // 1. Basename exact match
  if (bLower === qLower) {
    const indices: number[] = [];
    for (let i = 0; i < basename.length; i++) {
      indices.push(basenameStart + i);
    }
    const caseBonus = basename === trimmed ? 500 : 0;
    return {
      path,
      score: 25000 + caseBonus - Math.min(200, path.length),
      indices,
    };
  }

  // 2. Basename prefix match
  if (bLower.startsWith(qLower)) {
    const indices: number[] = [];
    for (let i = 0; i < trimmed.length; i++) {
      indices.push(basenameStart + i);
    }
    const caseBonus = basename.startsWith(trimmed) ? 300 : 0;
    return {
      path,
      score: 18000 + caseBonus - Math.min(200, path.length),
      indices,
    };
  }

  // 3. Basename substring match
  const bSubIdx = bLower.indexOf(qLower);
  if (bSubIdx !== -1) {
    const indices: number[] = [];
    for (let i = 0; i < trimmed.length; i++) {
      indices.push(basenameStart + bSubIdx + i);
    }
    const boundaryBonus = isBoundaryChar(basename[bSubIdx - 1] ?? "") ? 400 : 0;
    return {
      path,
      score: 12000 + boundaryBonus - bSubIdx * 10 - Math.min(200, path.length),
      indices,
    };
  }

  // 4. Path substring match
  const pSubIdx = pLower.indexOf(qLower);
  if (pSubIdx !== -1) {
    const indices: number[] = [];
    for (let i = 0; i < trimmed.length; i++) {
      indices.push(pSubIdx + i);
    }
    const inBasename = pSubIdx >= basenameStart;
    const boundaryBonus = pSubIdx === 0 || isBoundaryChar(path[pSubIdx - 1]) ? 300 : 0;
    return {
      path,
      score: 8000 + (inBasename ? 2000 : 0) + boundaryBonus - pSubIdx - Math.min(200, path.length),
      indices,
    };
  }

  // 5. Path segment prefix match (e.g. "crd/fil" matching "crates/repomon-daemon/src/files.rs")
  if (trimmed.includes("/")) {
    const qSegments = trimmed.split("/").filter(Boolean);
    const pSegments = path.split("/");
    let qSegIdx = 0;
    let currPathOffset = 0;
    const segmentIndices: number[] = [];
    let allSegmentsMatched = true;

    for (let i = 0; i < pSegments.length && qSegIdx < qSegments.length; i++) {
      const pSeg = pSegments[i];
      const qSeg = qSegments[qSegIdx];
      const pSegLower = pSeg.toLowerCase();
      const qSegLower = qSeg.toLowerCase();

      if (pSegLower.startsWith(qSegLower)) {
        for (let c = 0; c < qSeg.length; c++) {
          segmentIndices.push(currPathOffset + c);
        }
        qSegIdx++;
      }
      currPathOffset += pSeg.length + 1;
    }

    if (qSegIdx === qSegments.length && allSegmentsMatched) {
      return {
        path,
        score: 6000 - Math.min(200, path.length),
        indices: segmentIndices,
      };
    }
  }

  // 6. Subsequence match
  const matchedIndices: number[] = [];
  let pIdx = 0;
  let qIdx = 0;
  let score = 2000;
  let consecutiveCount = 0;

  while (qIdx < qLower.length && pIdx < pLower.length) {
    const qChar = qLower[qIdx];
    const pChar = pLower[pIdx];

    if (qChar === pChar) {
      matchedIndices.push(pIdx);

      // Basename match bonus
      if (pIdx >= basenameStart) {
        score += 80;
      }

      // Word boundary bonus
      if (pIdx === 0 || isBoundaryChar(path[pIdx - 1])) {
        score += 50;
      } else if (path[pIdx] !== path[pIdx].toLowerCase() && path[pIdx - 1] === path[pIdx - 1].toLowerCase()) {
        // CamelCase boundary
        score += 40;
      }

      // Consecutive bonus
      if (consecutiveCount > 0) {
        score += consecutiveCount * 25;
      }
      consecutiveCount++;

      // Exact case bonus
      if (path[pIdx] === trimmed[qIdx]) {
        score += 10;
      }

      qIdx++;
    } else {
      consecutiveCount = 0;
    }
    pIdx++;
  }

  if (qIdx < qLower.length) {
    return null;
  }

  // Length and span penalty
  const firstMatch = matchedIndices[0] ?? 0;
  const lastMatch = matchedIndices[matchedIndices.length - 1] ?? 0;
  const span = lastMatch - firstMatch;
  score -= span * 2;
  score -= Math.min(200, path.length);

  return {
    path,
    score: Math.max(1, score),
    indices: matchedIndices,
  };
}

export function filterAndRankPaths(
  paths: string[],
  query: string,
  limit = 50,
): FuzzyMatch[] {
  const matches: FuzzyMatch[] = [];
  for (const path of paths) {
    const match = scorePath(path, query);
    if (match !== null) {
      matches.push(match);
    }
  }

  matches.sort((a, b) => {
    if (b.score !== a.score) {
      return b.score - a.score;
    }
    if (a.path.length !== b.path.length) {
      return a.path.length - b.path.length;
    }
    return a.path.localeCompare(b.path);
  });

  return matches.slice(0, limit);
}
