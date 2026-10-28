import { createContext, useContext } from 'react';
import { StackActions } from '@react-navigation/native';
import { navigationRef } from './navigationRef';
import type { RootStackParamList } from './routes';

/** The pages a connection opens. */
export type ConnectRoute = 'Session' | 'AdapterSession' | 'EnableSharing';

/**
 * Where a connection opens its page. The main window's navigator by
 * default; a 选电脑 window of its own  opens it in that window.
 */
export type ConnectNavigator = {
  navigate<R extends ConnectRoute>(name: R, params: RootStackParamList[R]): void;
  replace<R extends ConnectRoute>(name: R, params: RootStackParamList[R]): void;
};

type Navigate = (
  this: typeof navigationRef,
  name: ConnectRoute,
  params: object,
) => void;

export const mainConnectNavigator: ConnectNavigator = {
  navigate(name, params) {
    (navigationRef.navigate as unknown as Navigate).call(
      navigationRef,
      name,
      params,
    );
  },
  replace(name, params) {
    navigationRef.dispatch(StackActions.replace(name, params));
  },
};

export const ConnectNavigatorContext =
  createContext<ConnectNavigator>(mainConnectNavigator);

export function useConnectNavigator(): ConnectNavigator {
  return useContext(ConnectNavigatorContext);
}
