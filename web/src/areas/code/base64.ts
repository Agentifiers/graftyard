// GHES sends file content as base64 of the raw bytes, wrapped at 60 columns. These helpers
// convert between that and text, for the mock and for the UI.

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary);
}

export function base64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64.replace(/\s+/g, ""));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

export const encodeText = (text: string) => bytesToBase64(new TextEncoder().encode(text));

// Null when the bytes are not UTF-8 text, as for an image.
export function decodeText(b64: string): string | null {
  const bytes = base64ToBytes(b64);
  if (bytes.includes(0)) return null;
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}

export const wrap60 = (b64: string) => b64.replace(/(.{60})/g, "$1\n");
