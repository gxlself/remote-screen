export const fontFamily = {
  sans: {
    400: 'Geist-Regular',
    500: 'Geist-Medium',
    600: 'Geist-SemiBold',
    700: 'Geist-Bold',
  },
  mono: {
    400: 'GeistMono-Regular',
    500: 'GeistMono-Medium',
    600: 'GeistMono-SemiBold',
  },
} as const;

export type Weight = 400 | 500 | 600 | 700;

type TypeVariant =
  | 'largeTitle'
  | 'title'
  | 'headline'
  | 'body'
  | 'bodyStrong'
  | 'section'
  | 'footnote'
  | 'caption'
  | 'tiny'
  | 'button'
  | 'mono'
  | 'monoSmall';

type TypeSpec = {
  fontSize: number;
  lineHeight: number;
  weight: Weight;
  letterSpacing?: number;
};

const typeSpecs = {
  largeTitle: { fontSize: 34, lineHeight: 40, weight: 700 },
  title: { fontSize: 28, lineHeight: 34, weight: 700 },
  headline: { fontSize: 17, lineHeight: 22, weight: 600 },
  body: { fontSize: 15, lineHeight: 22, weight: 400 },
  bodyStrong: { fontSize: 15, lineHeight: 22, weight: 600 },
  section: { fontSize: 13, lineHeight: 19, weight: 600, letterSpacing: 0.52 },
  footnote: { fontSize: 13, lineHeight: 19, weight: 400 },
  caption: { fontSize: 12, lineHeight: 17, weight: 400 },
  tiny: { fontSize: 11, lineHeight: 14, weight: 500 },
  button: { fontSize: 14, lineHeight: 20, weight: 600 },
  mono: { fontSize: 13, lineHeight: 18, weight: 400 },
  monoSmall: { fontSize: 12, lineHeight: 17, weight: 400 },
} as const satisfies Record<TypeVariant, TypeSpec>;

export type TypeVariantName = keyof typeof typeSpecs;

export function fontFor(weight: Weight, mono = false): string {
  if (mono) {
    return fontFamily.mono[weight === 700 ? 600 : weight];
  }
  return fontFamily.sans[weight];
}

export function textStyle(variant: TypeVariantName, weight?: Weight) {
  const spec = typeSpecs[variant];
  const effectiveWeight = weight ?? spec.weight;
  return {
    fontSize: spec.fontSize,
    lineHeight: spec.lineHeight,
    fontFamily: fontFor(
      effectiveWeight,
      variant === 'mono' || variant === 'monoSmall',
    ),
    ...('letterSpacing' in spec ? { letterSpacing: spec.letterSpacing } : {}),
  };
}
