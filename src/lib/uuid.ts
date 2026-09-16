/**
 * UUID v7 (time-ordered) — matches the ids the Rust core generates
 * (`ids::new_id`), so client-created rows sort naturally with server ones.
 */

export function newId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  const ms = BigInt(Date.now());
  for (let i = 0; i < 6; i++) {
    bytes[i] = Number((ms >> BigInt(8 * (5 - i))) & 0xffn);
  }
  bytes[6] = (bytes[6] & 0x0f) | 0x70; // version 7
  bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant
  const hex = [...bytes]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}