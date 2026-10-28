import type { ReactNode } from 'react';
import { SafeAreaProvider } from 'react-native-safe-area-context';
import DataProviders from '../features/data/DataProviders';
import EventsProvider from '../features/events/EventsProvider';
import { CoreFilesProvider } from '../features/files/CoreFilesProvider';

export default function AppProviders({ children }: { children: ReactNode }) {
  return (
    <SafeAreaProvider>
      <DataProviders>
        <EventsProvider>
          <CoreFilesProvider>{children}</CoreFilesProvider>
        </EventsProvider>
      </DataProviders>
    </SafeAreaProvider>
  );
}
