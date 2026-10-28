import { DarkTheme, type Theme } from '@react-navigation/native';
import { tokens } from '../../theme/tokens';

export const navTheme: Theme = {
  ...DarkTheme,
  colors: {
    ...DarkTheme.colors,
    primary: tokens.accent,
    background: tokens.bg,
    card: tokens.card,
    text: tokens.text,
    border: tokens.cardLine,
    notification: tokens.bad,
  },
};
