/**
 * @format
 */

import './src/features/diagnostics/earlyEntry';
import { AppRegistry } from 'react-native';
import App from './App';
import WindowRoot from './src/app/WindowRoot';
import { name as appName } from './app.json';

AppRegistry.registerComponent(appName, () => App);
// a computer in a window of its own, a second surface of the same
// React host (SceneDelegate / RsReactRuntime).
AppRegistry.registerComponent('RemoteScreenWindow', () => WindowRoot);
