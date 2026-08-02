/** base64 文字列を Uint8Array に戻す。xterm.js はバイト列を直接 write できる */
export function decodeBase64(input: string): Uint8Array {
  const binary = atob(input);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

/** xterm.js の onBinary が返す「1 文字 = 1 バイト」文字列を base64 にする */
export function encodeBinaryString(input: string): string {
  let binary = '';
  for (let i = 0; i < input.length; i += 1) {
    binary += String.fromCharCode(input.charCodeAt(i) & 0xff);
  }
  return btoa(binary);
}
