export type LaunchOptions = {
  dataDir?: string;
  autoConnect?: string;
  disconnectAfterS?: number;
  selfTestMove: boolean;
  devHarness?: string;
  harnessArg?: string;
};

function stringProp(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim().length > 0
    ? value
    : undefined;
}

export function parseLaunchProps(
  props: Record<string, unknown>,
): LaunchOptions {
  const disconnectAfter = stringProp(props.disconnectAfter);
  const seconds =
    disconnectAfter !== undefined && /^\d+$/.test(disconnectAfter)
      ? Number(disconnectAfter)
      : NaN;

  return {
    dataDir: stringProp(props.dataDir),
    autoConnect: stringProp(props.autoConnect),
    disconnectAfterS:
      Number.isInteger(seconds) && seconds >= 1 && seconds <= 86400
        ? seconds
        : undefined,
    selfTestMove: /^(yes|1|true)$/i.test(stringProp(props.selfTestMove) ?? ''),
    devHarness: stringProp(props.devHarness),
    harnessArg: stringProp(props.harnessArg),
  };
}

let launchOptions: LaunchOptions | undefined;

export function setLaunchOptions(options: LaunchOptions): void {
  launchOptions = options;
}

export function getLaunchOptions(): LaunchOptions {
  return launchOptions ?? parseLaunchProps({});
}
