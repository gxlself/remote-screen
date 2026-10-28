import { createNavigationContainerRef } from '@react-navigation/native';
import type { RootStackParamList } from './routes';

export const navigationRef = createNavigationContainerRef<RootStackParamList>();
