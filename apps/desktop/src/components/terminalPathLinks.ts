export interface PathRefMatch {
  raw: string;
  path: string;
  line?: number;
  column?: number;
  startIndex: number;
  endIndex: number;
}

export interface FindPathRefsOptions {
  platform?: string;
}

export function isMacPlatform(platformOverride?: string): boolean {
  if (platformOverride !== undefined) {
    return platformOverride === "darwin" || platformOverride === "mac" || platformOverride === "macos";
  }
  if (typeof navigator !== "undefined") {
    const nav = navigator as { userAgentData?: { platform?: string }; platform?: string; userAgent?: string };
    const p = nav.userAgentData?.platform || nav.platform || nav.userAgent || "";
    return /mac|darwin|iphone|ipad|ipod/i.test(p);
  }
  if (typeof process !== "undefined" && process.platform) {
    return process.platform === "darwin";
  }
  return true;
}

const TRAILING_PUNCTUATION_REGEX = /[.,:;!?'"`\)\]}>]+$/;
const LEADING_PUNCTUATION_REGEX = /^[('"`<\[{]+/;

const CANDIDATE_TOKEN_REGEX = /(?:-->\s*|at\s+(?:[a-zA-Z0-9_$.<>]+\s+\()?)?([a-zA-Z]:[\\/][^\s"'`<>()[\]]+|(?:\.{1,2}[\\/]|[\\/]|[a-zA-Z0-9_~-][a-zA-Z0-9_.~-]*[\\/])[^\s"'`<>()[\]]+|[a-zA-Z0-9_~-][a-zA-Z0-9_.~-]*\.[a-zA-Z0-9_-]+:\d+(?::\d+)?(?:[^\s"'`<>()[\]]*))/g;

export function findPathRefs(line: string, options?: FindPathRefsOptions): PathRefMatch[] {
  const isMac = isMacPlatform(options?.platform);
  const results: PathRefMatch[] = [];

  CANDIDATE_TOKEN_REGEX.lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = CANDIDATE_TOKEN_REGEX.exec(line)) !== null) {
    const fullMatch = match[0];
    const candidateGroup = match[1];
    if (!candidateGroup) continue;

    const candidateOffset = fullMatch.indexOf(candidateGroup);
    const candidateStartIndexInLine = match.index + candidateOffset;

    // Check if preceded by a URL protocol (http://, https://, file://)
    const lineBefore = line.slice(0, candidateStartIndexInLine);
    if (/https?:\/\/[\w.\-~:/]*$/i.test(lineBefore) || /file:\/\/[\w.\-~:/]*$/i.test(lineBefore)) {
      continue;
    }

    let cleaned = candidateGroup;
    let leadingTrim = 0;
    const leadMatch = cleaned.match(LEADING_PUNCTUATION_REGEX);
    if (leadMatch) {
      leadingTrim = leadMatch[0].length;
      cleaned = cleaned.slice(leadingTrim);
    }

    const trailMatch = cleaned.match(TRAILING_PUNCTUATION_REGEX);
    if (trailMatch) {
      cleaned = cleaned.slice(0, cleaned.length - trailMatch[0].length);
    }

    if (!cleaned) continue;

    let filePath = cleaned;
    let lineNumber: number | undefined;
    let columnNumber: number | undefined;

    const lineColMatch = filePath.match(/:(\d+):(\d+)$/);
    if (lineColMatch) {
      lineNumber = parseInt(lineColMatch[1], 10);
      columnNumber = parseInt(lineColMatch[2], 10);
      filePath = filePath.slice(0, filePath.length - lineColMatch[0].length);
    } else {
      const lineMatch = filePath.match(/:(\d+)$/);
      if (lineMatch) {
        lineNumber = parseInt(lineMatch[1], 10);
        filePath = filePath.slice(0, filePath.length - lineMatch[0].length);
      }
    }

    // File path must have an extension: e.g. .rs, .ts, .tsx, .js, .json, .md, etc.
    const extMatch = filePath.match(/\.([a-zA-Z0-9_-]+)$/);
    if (!extMatch) {
      continue;
    }

    // On macOS, Windows drive paths (e.g. C:\... or C:/...) must be ignored
    if (isMac && /^[a-zA-Z]:[\\/]/.test(filePath)) {
      continue;
    }

    // Must have at least one slash OR have a line number (to avoid bare filenames like `test.sh` in text)
    const hasSlash = /[\\/]/.test(filePath);
    if (!hasSlash && lineNumber === undefined) {
      continue;
    }

    const startIdx = candidateStartIndexInLine + leadingTrim;
    const endIdx = startIdx + cleaned.length;
    const raw = line.slice(startIdx, endIdx);

    results.push({
      raw,
      path: filePath,
      line: lineNumber,
      column: columnNumber,
      startIndex: startIdx,
      endIndex: endIdx,
    });
  }

  return results;
}
