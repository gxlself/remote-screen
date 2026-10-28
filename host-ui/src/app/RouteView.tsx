import { useEffect } from 'react';

import { useRoute } from './router';
import { ROUTES } from './routes';

export function RouteView() {
  const route = useRoute();
  const entry = ROUTES[route.page];
  const Page = entry.component;

  useEffect(() => {
    document.title = `Remote Host · ${entry.title}`;
    document.documentElement.dataset.page = route.page;
  }, [entry.title, route.page]);

  return <Page route={route} />;
}
