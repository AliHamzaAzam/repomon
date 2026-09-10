import { describe, expect, it } from "vitest";
import { attachmentTextParts } from "./attachmentText";
import { attachmentPrompt } from "./controls/AttachmentComposer";
describe("attachment delivery display", () => {
  it("round trips paths with spaces and quotes without displaying them as prose", () => {
    const path = '/Users/me/Application Support/a "quoted" image.png';
    const parts = attachmentTextParts(attachmentPrompt("Look here", [{path,name:"image.png"}]));
    expect(parts).toContainEqual({attachment:{path,name:'a "quoted" image.png'}});
    expect(parts.filter((part) => "text" in part).map((part) => part.text).join("")).toBe("Look here\n");
  });
  it("preserves fenced examples, malformed lines and unrelated prose", () => {
    const text = '```text\nAttached file: "/tmp/example.png"\n```\nAttached file: not JSON\nAn Attached file: "/tmp/example.png"';
    expect(attachmentTextParts(text)).toEqual([{text}]);
  });
  it("recognizes Windows paths but never web URLs", () => {
    const path = 'C:\\Users\\me\\photo.png';
    expect(attachmentTextParts(`Attached file: ${JSON.stringify(path)}`)).toEqual([{attachment:{path,name:"photo.png"}}]);
    const text = 'Attached file: "https://example.com/image.png"';
    expect(attachmentTextParts(text)).toEqual([{text}]);
  });
});
