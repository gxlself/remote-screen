/// <reference types="node" />
import { readdirSync, readFileSync } from 'fs';
import path from 'path';
import ts from 'typescript';
import type { RenderResult } from '@testing-library/react-native';
import { StyleSheet } from 'react-native';

export const srcRoot = path.resolve(__dirname, '../..');

export function sourceFiles(directory: string): string[] {
  return readdirSync(path.join(srcRoot, directory), { withFileTypes: true })
    .flatMap(entry => {
      const relative = `${directory}/${entry.name}`;
      if (relative === 'screens/dev') {
        return [];
      }
      return entry.isDirectory()
        ? sourceFiles(relative)
        : /\.(ts|tsx)$/.test(entry.name) && !/\.test\./.test(entry.name)
        ? [relative]
        : [];
    })
    .sort();
}

export function readSource(file: string): string {
  return readFileSync(path.join(srcRoot, file), 'utf8');
}

export function parseSource(file: string, source = readSource(file)) {
  return ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true);
}

// Preserve string/regexp/template/JSX text and every newline, so URLs and
// comment-like user text cannot conceal a violation or shift its line number.
export function stripComments(source: string): string {
  const parsed = parseSource('source.tsx', source);
  const literals: { start: number; end: number }[] = [];
  function visit(node: ts.Node) {
    if (
      ts.isStringLiteralLike(node) ||
      ts.isRegularExpressionLiteral(node) ||
      ts.isTemplateHead(node) ||
      ts.isTemplateMiddle(node) ||
      ts.isTemplateTail(node) ||
      ts.isJsxText(node)
    ) {
      literals.push({ start: node.getStart(parsed), end: node.end });
    } else {
      ts.forEachChild(node, visit);
    }
  }
  visit(parsed);
  const protectedRanges = new Map(
    literals.map(range => [range.start, range.end]),
  );
  let result = '';
  let index = 0;
  while (index < source.length) {
    const end = protectedRanges.get(index);
    if (end !== undefined && end > index) {
      result += source.slice(index, end);
      index = end;
    } else if (source.startsWith('//', index)) {
      const newline = source.indexOf('\n', index);
      const stop = newline === -1 ? source.length : newline;
      result += source.slice(index, stop).replace(/[^\r\n]/g, ' ');
      index = stop;
    } else if (source.startsWith('/*', index)) {
      const close = source.indexOf('*/', index + 2);
      const stop = close === -1 ? source.length : close + 2;
      result += source.slice(index, stop).replace(/[^\r\n]/g, ' ');
      index = stop;
    } else {
      result += source[index++];
    }
  }
  return result;
}

type TestNode = NonNullable<RenderResult['root']>;
const controlRoles = new Set([
  'button',
  'switch',
  'radio',
  'tab',
  'link',
  'adjustable',
]);

export function subtreeText(node: TestNode): string {
  if (
    node.props.accessibilityElementsHidden ||
    node.props.importantForAccessibility === 'no-hide-descendants'
  ) {
    return '';
  }
  return node.children
    .map(child =>
      typeof child === 'string'
        ? node.type === 'Text'
          ? child
          : ''
        : subtreeText(child),
    )
    .join(' ')
    .trim();
}

export function interactiveElements(root: TestNode | null): TestNode[] {
  const elements: TestNode[] = [];
  function visit(node: TestNode) {
    // RNTL 14 exposes host elements. Pressable consumes onPress and forwards
    // Pressability's onClick, so include that callback to catch role-less icons.
    if (
      controlRoles.has(node.props.accessibilityRole) ||
      ((typeof node.props.onPress === 'function' ||
        typeof node.props.onClick === 'function') &&
        node.props.accessible !== false)
    ) {
      elements.push(node);
    }
    node.children.forEach(child => {
      if (typeof child !== 'string') {
        visit(child);
      }
    });
  }
  if (root != null) {
    visit(root);
  }
  return elements;
}

export function missingLabels(root: TestNode | null): string[] {
  return interactiveElements(root)
    .filter(
      node =>
        !(
          typeof node.props.accessibilityLabel === 'string' &&
          node.props.accessibilityLabel.trim()
        ) && !subtreeText(node),
    )
    .map(
      node =>
        `${
          node.props.testID ??
          (subtreeText(node).slice(0, 60) || '(no text/testID)')
        } (${node.props.accessibilityRole ?? 'onPress'})`,
    );
}

export function expectLabels(root: TestNode | null) {
  expect(interactiveElements(root).length).toBeGreaterThan(0);
  expect(missingLabels(root)).toEqual([]);
}

/** Only host nodes in the accessibility tree, including disabled controls. */
export function visibleElements(root: TestNode | null): TestNode[] {
  if (
    root === null ||
    root.props.accessibilityElementsHidden ||
    root.props.importantForAccessibility === 'no-hide-descendants'
  ) {
    return [];
  }
  return [
    root,
    ...root.children.flatMap(child =>
      typeof child === 'string' ? [] : visibleElements(child),
    ),
  ];
}

export function controlDescription(node: TestNode): string {
  return node.props.testID ?? (subtreeText(node).slice(0, 60) || '(无)');
}

export function visibleControls(root: TestNode | null): TestNode[] {
  const visible = new Set(visibleElements(root));
  return interactiveElements(root).filter(node => visible.has(node));
}

export function swallowedControls(
  root: TestNode | null,
): { control: string; container: string }[] {
  const controls = new Set(interactiveElements(root));
  const swallowed: { control: string; container: string }[] = [];
  function visit(node: TestNode, container?: TestNode) {
    if (
      node.props.accessibilityElementsHidden ||
      node.props.importantForAccessibility === 'no-hide-descendants'
    ) {
      return;
    }
    if (controls.has(node) && container !== undefined) {
      swallowed.push({
        control: controlDescription(node),
        container: controlDescription(container),
      });
    }
    const ancestor = node.props.accessible === true ? node : container;
    node.children.forEach(child => {
      if (typeof child !== 'string') {
        visit(child, ancestor);
      }
    });
  }
  if (root !== null) {
    visit(root);
  }
  return swallowed;
}

/** Unknown/content-sized dimensions stay unknown; hitSlop cannot size them. */
export function touchSize(node: TestNode): {
  width: number | undefined;
  height: number | undefined;
} {
  const style = StyleSheet.flatten(node.props.style) ?? {};
  const slop = node.props.hitSlop;
  function inset(edge: 'top' | 'bottom' | 'left' | 'right'): number {
    return typeof slop === 'number' ? slop : slop?.[edge] ?? 0;
  }
  function dimension(size: unknown, min: unknown): number | undefined {
    const values = [size, min].filter(
      (value): value is number => typeof value === 'number',
    );
    return values.length === 0 ? undefined : Math.max(...values);
  }
  const width = dimension(style.width, style.minWidth);
  const height = dimension(style.height, style.minHeight);
  return {
    width:
      width === undefined ? undefined : width + inset('left') + inset('right'),
    height:
      height === undefined
        ? undefined
        : height + inset('top') + inset('bottom'),
  };
}

export type ControlException = {
  page: string;
  control: string;
  reason: string;
};

/** Match exact findings, and account for every exception at suite completion. */
export function withoutAllowed<T extends { reason: string }>(
  findings: Omit<T, 'reason'>[],
  allowed: readonly T[],
  used: Set<T>,
): Omit<T, 'reason'>[] {
  return findings.filter(finding => {
    const exception = allowed.find(item =>
      Object.entries(finding).every(
        ([key, value]) => item[key as keyof T] === value,
      ),
    );
    if (exception === undefined) {
      return true;
    }
    expect(exception.reason.trim()).not.toBe('');
    used.add(exception);
    return false;
  });
}
