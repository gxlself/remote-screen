export const tailnet = {
  controlUrl: {
    title: 'Headscale 地址',
    label: 'Headscale 地址',
    placeholder: 'https://hs.example.com',
    note: '继续后会打开浏览器，用 Headscale 上设好的登录方式（比如公司账号）登录。',
    saveOnlyNote: '保存后，用 App 内连接时会连这个地址，并打开浏览器登录。',
    invalid: '地址不对，填 Headscale 的网址，例如 https://hs.example.com',
    changeDialog: {
      title: '改 Headscale 地址？',
      message: '改了以后，App 内连接要在浏览器里重新登录。',
      confirm: '改地址',
    },
    clearDialog: {
      title: '清空 Headscale 地址？',
      message: '清空后，App 内连接和自动模式都要重新填地址、重新登录。',
      confirm: '清空',
    },
  },
} as const;
