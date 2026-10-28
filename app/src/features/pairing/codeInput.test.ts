import { applyKey, digitsOf, isCompleteCode, sanitizeCode } from './codeInput';

describe('pairing code input', () => {
  it.each([
    ['48a2-913x7', '482913'],
    ['482 913', '482913'],
    ['４８２', '482'],
    ['１２３４５６７', '123456'],
  ])('sanitizes %s to %s', (input, expected) => {
    expect(sanitizeCode(input)).toBe(expected);
  });

  it('removes the last digit on Backspace', () => {
    expect(applyKey('482', 'Backspace')).toBe('48');
  });

  it('appends a digit', () => {
    expect(applyKey('482', '9')).toBe('4829');
  });

  it('does not append past six digits', () => {
    expect(applyKey('482913', '1')).toBe('482913');
  });

  it('ignores keys other than digits and Backspace', () => {
    expect(applyKey('482', 'Enter')).toBe('482');
  });

  it('accepts exactly six digits as complete', () => {
    expect(isCompleteCode('482913')).toBe(true);
    expect(isCompleteCode('48291')).toBe(false);
    expect(isCompleteCode('48291a')).toBe(false);
  });

  it('pads the digits for the six code boxes', () => {
    expect(digitsOf('48')).toEqual(['4', '8', '', '', '', '']);
    expect(digitsOf('482913')).toEqual(['4', '8', '2', '9', '1', '3']);
  });
});
