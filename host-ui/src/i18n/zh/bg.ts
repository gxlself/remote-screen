const bg = {
  handoffTitle: (controller: string) => `${controller} 接管了控制`,
  handoffBody: (previousController: string) =>
    `${previousController} 现在只能观看。两台都能在菜单栏里单独断开。`,
  handoffShortBody: '可以在菜单栏里断开。',
  watchTitle: (viewer: string) => `${viewer} 加入了，正在观看`,
  watchBody: (controller: string, viewer: string) =>
    `${controller} 仍在控制这台 Mac。${viewer} 上点「接管控制」就能换过来。`,
  pairedTitle: '新设备配对成功',
  pairedBody: '可以在菜单栏 › 已配对的设备里修改它的权限。',
  pairedFileTitle: '新设备用配对文件完成了配对',
  pairedPasswordTitle: (account: string) => `新设备用账户 ${account} 完成了配对`,
  pairedPasswordNoAccountTitle: '新设备用账户完成了配对',
  pairedApproveTitle: (deviceName: string) => `${deviceName} 经你的其他设备批准加入`,
} as const;

export default bg;
