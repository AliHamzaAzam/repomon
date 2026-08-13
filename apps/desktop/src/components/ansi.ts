/// Strip terminal escape sequences from a pane capture so it reads as plain text.
///
/// The repomind pane arrives as a raw capture, escapes and all, and is rendered into a `<pre>`
/// rather than through xterm the way lane panes are. Without this the panel shows the literal
/// bytes: `\x1b[93m`, `\x1b[1m`, and OSC 8 hyperlink wrappers around every URL, which is what made
/// a trust prompt unreadable.
///
/// Handles the two families that actually appear here:
///   - CSI  `ESC [ ... final`      colour, bold, cursor moves
///   - OSC  `ESC ] ... BEL|ST`     hyperlinks (`ESC ] 8 ; id=x ; url ST`), title sets
/// The OSC payload is dropped but the link *text* between the two OSC 8 markers survives, because
/// it sits outside the escape and is what the reader actually wants.
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

/// Drop blank lines from the top and bottom of a pane capture.
///
/// A capture is the whole tmux pane, so a short message sits in a tall field of empty rows and the
/// panel renders mostly nothing. Only the edges are touched: blank lines *inside* the output are
/// the agent's own spacing and removing them would reflow its layout.
export function trimBlankEdges(text: string): string {
  const lines = text.split("\n");
  let start = 0;
  let end = lines.length;
  while (start < end && lines[start].trim() === "") start += 1;
  while (end > start && lines[end - 1].trim() === "") end -= 1;
  return lines.slice(start, end).join("\n");
}
