import { useEffect, useRef, useState } from 'react';
import { BackHandler, Platform, useWindowDimensions } from 'react-native';
import type { TabScreenProps } from '../../app/navigation/routes';
import { FallbackGuide } from '../../features/fallback/FallbackGuide';
import { HomeNarrow } from './HomeNarrow';
import { useHomeModel } from './useHomeModel';
import { useTabScrollToTop, type ScrollViewHandle } from './useTabScrollToTop';
import { HomeWide } from './wide/HomeWide';

export default function HomeScreen({ navigation }: TabScreenProps<'Home'>) {
  const [query, setQuery] = useState('');
  const { width } = useWindowDimensions();
  const scrollRef = useRef<ScrollViewHandle>(null);
  const { guide, closeGuide, ...model } = useHomeModel(query, navigation);
  const props = { ...model, query, setQuery, scrollRef };
  useTabScrollToTop(navigation, scrollRef);

  // Android back clears the search before it leaves the App.
  useEffect(() => {
    if (Platform.OS !== 'android' || query === '') {
      return;
    }
    const subscription = BackHandler.addEventListener(
      'hardwareBackPress',
      () => {
        if (!navigation.isFocused()) {
          return false;
        }
        setQuery('');
        return true;
      },
    );
    return () => subscription.remove();
  }, [navigation, query]);

  return (
    <>
      {width >= 700 ? <HomeWide {...props} /> : <HomeNarrow {...props} />}
      <FallbackGuide guide={guide} onClose={closeGuide} />
    </>
  );
}
