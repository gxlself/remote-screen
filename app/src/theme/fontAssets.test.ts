import { existsSync, readFileSync } from 'fs';
import path from 'path';
import { fontFamily } from './typography';

const androidFonts = path.resolve(
  __dirname,
  '../../android/app/src/main/assets/fonts',
);
const iosFonts = path.resolve(__dirname, '../../ios/RemoteScreen/Fonts');

const fontNames = [
  ...Object.values(fontFamily.sans),
  ...Object.values(fontFamily.mono),
];

describe('Android font assets', () => {
  it.each(fontNames)('bundles %s.ttf identically to iOS', name => {
    const android = readFileSync(path.join(androidFonts, `${name}.ttf`));
    const ios = readFileSync(path.join(iosFonts, `${name}.ttf`));

    expect(android.equals(ios)).toBe(true);
  });

  it('bundles the font license', () => {
    expect(existsSync(path.join(androidFonts, 'OFL-1.1.txt'))).toBe(true);
  });
});
