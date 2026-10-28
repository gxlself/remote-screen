import { Fragment, useState } from 'react';
import { Platform } from 'react-native';
import { Banner } from '../../components/Banner';
import { Button } from '../../components/Button';
import { ControlUrlSheet } from '../../features/tailnet/ControlUrlSheet';
import { home } from '../../i18n/zh/home';
import { VpnInfoSheet } from './VpnInfoSheet';
import type { HomeBanner } from './useHomeModel';

export type HomeBannersProps = {
  banners: HomeBanner[];
};

export function HomeBanners({ banners }: HomeBannersProps) {
  const [open, setOpen] = useState(false);

  if (banners.length === 0) {
    return null;
  }

  return (
    <>
      {banners.map(banner => {
        // Without the App's own connection there is nothing to switch to;
        // the sheet explains what to do instead.
        const action: HomeBanner['action'] =
          banner.id === 'vpnConflict' && banner.action === undefined
            ? { label: home.androidVpn.help, onPress: () => setOpen(true) }
            : banner.action;
        return (
          <Fragment key={banner.id}>
            <Banner
              action={
                action === undefined
                  ? undefined
                  : {
                      label: action.label,
                      onPress: action.onPress,
                      loading: action.loading,
                    }
              }
              icon={
                banner.id === 'tailnetDown'
                  ? 'wifi'
                  : banner.id === 'embeddedBoth'
                  ? 'check'
                  : 'warning'
              }
              message={banner.message}
              announce={banner.announce}
              onDismiss={banner.onDismiss}
              testID={`home-banner-${banner.id}`}
              title={banner.title}
              tone={banner.tone}
            />
            {banner.secondary === undefined ? null : (
              <Button
                align="start"
                onPress={banner.secondary.onPress}
                testID={`home-banner-${banner.id}-secondary`}
                title={banner.secondary.label}
                variant="text"
              />
            )}
            {Platform.OS === 'android' &&
            banner.id === 'vpnConflict' &&
            banner.onUseEmbedded !== undefined ? (
              <Button
                align="start"
                onPress={() => setOpen(true)}
                testID="home-banner-vpnConflict-why"
                title={home.embedded.why}
                variant="text"
              />
            ) : null}
            {banner.controlUrlSheet === undefined ? null : (
              <ControlUrlSheet
                onClose={banner.controlUrlSheet.onClose}
                onDone={banner.controlUrlSheet.onDone}
              />
            )}
          </Fragment>
        );
      })}
      <VpnInfoSheet
        onClose={() => setOpen(false)}
        onUseEmbedded={
          banners.find(banner => banner.id === 'vpnConflict')?.onUseEmbedded
        }
        visible={open}
      />
    </>
  );
}
