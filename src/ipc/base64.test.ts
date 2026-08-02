import { describe, expect, it } from 'vitest';
import { decodeBase64, encodeBinaryString } from './base64';

describe('decodeBase64', () => {
  it('ASCII を Uint8Array に戻す', () => {
    expect(Array.from(decodeBase64('aGVsbG8='))).toEqual([104, 101, 108, 108, 111]);
  });

  it('マルチバイトの UTF-8 バイト列をそのまま返す', () => {
    // "あ" = E3 81 82
    expect(Array.from(decodeBase64('44GC'))).toEqual([0xe3, 0x81, 0x82]);
  });

  it('空文字列は空配列になる', () => {
    expect(decodeBase64('').length).toBe(0);
  });

  it('0x00 を含むバイト列を落とさない', () => {
    expect(Array.from(decodeBase64('AAEC'))).toEqual([0x00, 0x01, 0x02]);
  });
});

describe('encodeBinaryString', () => {
  it('1 文字 = 1 バイトの文字列を base64 にする', () => {
    expect(encodeBinaryString('hello')).toBe('aGVsbG8=');
  });

  it('0x80 以上のバイトを保持する', () => {
    const binary = String.fromCharCode(0x1b, 0x5b, 0x4d, 0xe3);
    expect(Array.from(decodeBase64(encodeBinaryString(binary)))).toEqual([0x1b, 0x5b, 0x4d, 0xe3]);
  });

  it('0xff を超える符号位置を下位 1 バイトに切り詰める', () => {
    expect(Array.from(decodeBase64(encodeBinaryString(String.fromCharCode(0x141))))).toEqual([
      0x41,
    ]);
  });
});
