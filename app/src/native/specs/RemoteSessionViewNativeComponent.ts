import { codegenNativeCommands, codegenNativeComponent } from 'react-native';
import type React from 'react';
import type { CodegenTypes, HostComponent, ViewProps } from 'react-native';

export type VideoStats = Readonly<{
  fps: CodegenTypes.Int32;
  decodeP50Ms: CodegenTypes.Float;
  decodeP95Ms: CodegenTypes.Float;
  latencyP50Ms: CodegenTypes.Float; // -1 until clock synchronization is available.
  queue: CodegenTypes.Int32;
  dropped: CodegenTypes.Int32;
  keyframeRequests: CodegenTypes.Int32;
  width: CodegenTypes.Int32;
  height: CodegenTypes.Int32;
  codec: string;
}>;

export interface NativeProps extends ViewProps {
  sessionId?: CodegenTypes.WithDefault<string, ''>; // Decimal u64; empty detaches.
  mode?: CodegenTypes.WithDefault<'trackpad' | 'touch', 'trackpad'>;
  keyboardInset?: CodegenTypes.WithDefault<CodegenTypes.Float, 0>;
  toolbarCollapsed?: CodegenTypes.WithDefault<boolean, false>;
  reduceMotion?: CodegenTypes.WithDefault<boolean, false>;
  cursorSpeed?: CodegenTypes.WithDefault<CodegenTypes.Float, 1.4>;
  naturalScroll?: CodegenTypes.WithDefault<boolean, true>;
  haptics?: CodegenTypes.WithDefault<boolean, true>;
  /** the open computers in swipe order, and their names. */
  spaceIds?: ReadonlyArray<string>;
  spaceNames?: ReadonlyArray<string>;
  /** Show this session's last picture until its first frame arrives. */
  showSnapshot?: CodegenTypes.WithDefault<boolean, false>;
  onGesture?: CodegenTypes.DirectEventHandler<Readonly<{ kind: string }>>;
  onPointerLock?: CodegenTypes.DirectEventHandler<
    Readonly<{ mouse: boolean; locked: boolean; reason: string }>
  >;
  onViewport?: CodegenTypes.DirectEventHandler<
    Readonly<{
      zoom: CodegenTypes.Float;
      x: CodegenTypes.Float;
      y: CodegenTypes.Float;
    }>
  >;
  onFirstFrame?: CodegenTypes.DirectEventHandler<
    Readonly<{
      epoch: CodegenTypes.Int32;
      width: CodegenTypes.Int32;
      height: CodegenTypes.Int32;
    }>
  >;
  onVideoStats?: CodegenTypes.DirectEventHandler<VideoStats>;
  /** A three-finger swipe between computers began or ended. */
  onSpaceSwipe?: CodegenTypes.DirectEventHandler<Readonly<{ phase: string }>>;
  /** The slide to another computer finished: switch to it now. */
  onSpaceSwitch?: CodegenTypes.DirectEventHandler<
    Readonly<{ sessionId: string }>
  >;
}

export interface NativeCommands {
  zoomReset(r: React.ElementRef<HostComponent<NativeProps>>): void;
  centerCursor(r: React.ElementRef<HostComponent<NativeProps>>): void;
  showMagnifier(
    r: React.ElementRef<HostComponent<NativeProps>>,
    on: boolean,
  ): void;
  /** Slides to `sessionId` ('next' / 'previous'), then onSpaceSwitch. */
  animateSpace(
    r: React.ElementRef<HostComponent<NativeProps>>,
    sessionId: string,
    direction: string,
  ): void;
}

export const Commands = codegenNativeCommands<NativeCommands>({
  supportedCommands: [
    'zoomReset',
    'centerCursor',
    'showMagnifier',
    'animateSpace',
  ],
});

export default codegenNativeComponent<NativeProps>(
  'RemoteSessionView',
) as HostComponent<NativeProps>;
