const capsule = {
  controlling: (name: string) => `${name} 正在控制这台 Mac`,
  watchingOne: (name: string) => `${name} 正在观看这台 Mac`,
  watchingMany: (count: number) => `${count} 台设备正在观看这台 Mac`,
  viewerOne: (name: string) => `${name} 在观看`,
  viewerMany: (count: number) => `${count} 台在观看`,
  recording: '录制中',
  recordingA11y: (who: string | number) =>
    typeof who === 'number'
      ? `${who} 台设备正在录制这台 Mac 的画面`
      : `${who} 正在录制这台 Mac 的画面`,
} as const;

export default capsule;
