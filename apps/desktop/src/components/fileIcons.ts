import type { Component } from "solid-js";

import {
  IconFile,
  IconFileBinary,
  IconFileCode,
  IconFileImage,
  IconFileText,
  type IconProps,
} from "./icons";

export function getFileIcon(path: string, kind?: string): Component<IconProps> {
  if (kind === "image") return IconFileImage;
  if (kind === "binary") return IconFileBinary;
  const ext = path.split(".").pop()?.toLowerCase();
  switch (ext) {
    case "rs":
    case "ts":
    case "tsx":
    case "js":
    case "jsx":
    case "go":
    case "py":
    case "c":
    case "cpp":
    case "h":
    case "css":
    case "html":
    case "sh":
    case "bash":
    case "zsh":
    case "json":
    case "toml":
    case "yaml":
    case "yml":
      return IconFileCode;
    case "md":
    case "txt":
    case "doc":
      return IconFileText;
    case "png":
    case "jpg":
    case "jpeg":
    case "gif":
    case "webp":
    case "svg":
    case "bmp":
    case "ico":
      return IconFileImage;
    default:
      return IconFile;
  }
}
