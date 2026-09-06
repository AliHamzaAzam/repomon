/// Removes CSI and OSC escapes from captured text while preserving hyperlink labels outside OSC
/// wrappers.
export function stripAnsi(input: string): string {
  let out = "";
  let i = 0;
  while (i < input.length) {
    const ch = input[i];
    if (ch !== "\x1b") {
      out += ch;
      i += 1;
      continue;
    }
    const next = input[i + 1];
    if (next === "[") {
      // CSI: parameter and intermediate bytes, then a final byte in @ through ~.
      let j = i + 2;
      while (j < input.length && !(input[j] >= "@" && input[j] <= "~")) j += 1;
      i = j + 1;
    } else if (next === "]") {
      // OSC: runs to BEL, or to ST (ESC \).
      let j = i + 2;
      while (j < input.length && input[j] !== "\x07" && !(input[j] === "\x1b" && input[j + 1] === "\\")) j += 1;
      i = input[j] === "\x1b" ? j + 2 : j + 1;
    } else {
      // A lone ESC or a two-byte sequence: drop the ESC and its selector.
      i += next === undefined ? 1 : 2;
    }
  }
  return out;
}

/// Trims empty capture edges without changing blank lines inside the output.
export function trimBlankEdges(text: string): string {
  const lines = text.split("\n");
  let start = 0;
  let end = lines.length;
  while (start < end && lines[start].trim() === "") start += 1;
  while (end > start && lines[end - 1].trim() === "") end -= 1;
  return lines.slice(start, end).join("\n");
}
