import { Component } from 'react';
import { requireNativeComponent, View } from 'react-native';
import type { ScrollViewProps } from 'react-native';

const RCTScrollView = requireNativeComponent<ScrollViewProps>('RCTScrollView');

// Keep the preset's host tree and ref methods without evaluating ScrollView's
// native implementation just to discover its name and Context static.
export default class ScrollViewMock extends Component<ScrollViewProps> {
  static Context =
    // eslint-disable-next-line @react-native/no-deep-imports
    require('react-native/Libraries/Components/ScrollView/ScrollViewContext').default;

  measure = jest.fn();
  measureInWindow = jest.fn();
  measureLayout = jest.fn();
  setNativeProps = jest.fn();
  focus = jest.fn();
  blur = jest.fn();
  getScrollResponder = jest.fn();
  getScrollableNode = jest.fn();
  getInnerViewNode = jest.fn();
  getInnerViewRef = jest.fn();
  getNativeScrollRef = jest.fn();
  scrollTo = jest.fn();
  scrollToEnd = jest.fn();
  flashScrollIndicators = jest.fn();
  scrollResponderZoomTo = jest.fn();
  scrollResponderScrollNativeHandleToKeyboard = jest.fn();

  render() {
    return (
      <RCTScrollView {...this.props}>
        {this.props.refreshControl}
        <View>{this.props.children}</View>
      </RCTScrollView>
    );
  }
}
