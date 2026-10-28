export const deepLink = {
  title: (hostName: string) => `配对到「${hostName}」？`,
  message: (addresses: string) =>
    `这个配对链接来自其他 App。确认是你自己的电脑再继续。地址：${addresses}`,
  cancel: '取消',
  pair: '配对',
} as const;
