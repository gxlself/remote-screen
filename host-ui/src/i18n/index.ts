import { detectHostOs } from '../platform/os';

import { zhFor } from './winText';

export const zh = zhFor(detectHostOs());
