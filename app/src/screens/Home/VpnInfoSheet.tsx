import { Modal, Platform, StyleSheet, View } from 'react-native';
import { BottomSheet } from '../../components/BottomSheet';
import { Button } from '../../components/Button';
import AppText from '../../components/AppText';
import { ListGroup, ListRow } from '../../components/List';
import { common } from '../../i18n/zh/common';
import { home } from '../../i18n/zh/home';
import { usePlatform } from '../../native/platform/PlatformContext';
import { openFirst, tailscaleUrls } from '../../features/platform/externalApps';

export type VpnInfoSheetProps = {
  visible: boolean;
  onClose: () => void;
  onUseEmbedded?: () => void;
};

export function VpnInfoSheet({
  visible,
  onClose,
  onUseEmbedded,
}: VpnInfoSheetProps) {
  if (!visible) {
    return null;
  }

  return (
    <VpnInfoSheetContent
      onClose={onClose}
      onUseEmbedded={onUseEmbedded}
      visible={visible}
    />
  );
}

function VpnInfoSheetContent({
  visible,
  onClose,
  onUseEmbedded,
}: VpnInfoSheetProps) {
  const platform = usePlatform();

  const openTailscale = () => {
    openFirst(platform, tailscaleUrls()).finally(onClose);
  };

  if (onUseEmbedded !== undefined) {
    const copy = home.embedded.sheet;
    return (
      <Modal
        animationType="fade"
        onRequestClose={onClose}
        transparent
        visible={visible}
      >
        <BottomSheet
          onClose={onClose}
          scroll
          testID="home-vpn-info"
          title={copy.title}
          footer={
            <View style={styles.footer}>
              <Button
                fullWidth
                onPress={onClose}
                testID="home-vpn-info-later"
                title={copy.later}
                variant="secondary"
              />
              <Button
                fullWidth
                onPress={() => {
                  onClose();
                  onUseEmbedded();
                }}
                testID="home-vpn-info-use-embedded"
                title={home.embedded.useEmbedded}
              />
            </View>
          }
        >
          <View style={styles.lines}>
            <AppText color="text2" variant="body">
              {copy.body}
            </AppText>
            <ListGroup>
              {copy.rows.map(row => (
                <ListRow
                  key={row.title}
                  subtitle={row.detail || undefined}
                  title={row.title}
                  value={row.value}
                />
              ))}
            </ListGroup>
            <View style={styles.footer}>
              <AppText weight={600}>{copy.workProfileTitle}</AppText>
              <AppText color="text2" variant="footnote">
                {copy.workProfileBody}
              </AppText>
            </View>
          </View>
        </BottomSheet>
      </Modal>
    );
  }

  const copy = Platform.OS === 'android' ? home.androidVpn : home.iosVpn;
  const lines: readonly string[] = copy.lines;
  return (
    <Modal
      animationType="fade"
      onRequestClose={onClose}
      transparent
      visible={visible}
    >
      <BottomSheet
        onClose={onClose}
        testID="home-vpn-info"
        title={copy.sheetTitle}
        footer={
          <View style={styles.footer}>
            <Button
              fullWidth
              onPress={onClose}
              testID="home-vpn-info-got-it"
              title={common.gotIt}
              variant="secondary"
            />
            <Button
              fullWidth
              onPress={openTailscale}
              testID="home-vpn-info-open-tailscale"
              title={home.banners.openTailscale}
            />
          </View>
        }
      >
        <View style={styles.lines}>
          {lines.map(line => (
            <AppText color="text2" key={line} variant="body">
              {line}
            </AppText>
          ))}
        </View>
      </BottomSheet>
    </Modal>
  );
}

const styles = StyleSheet.create({
  lines: { gap: 12 },
  footer: { gap: 8 },
});

export default VpnInfoSheet;
