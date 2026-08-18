import { expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { compileStyle, parse } from '@vue/compiler-sfc';

const filename = fileURLToPath(
  new URL('../src/views/settings.vue', import.meta.url)
);

test('Windows 和 Linux 设置选择器使用自定义 Select 组件', () => {
  const { descriptor } = parse(readFileSync(filename, 'utf8'), { filename });
  const style = descriptor.styles.find(block => block.scoped);
  if (!style || style.lang !== 'scss') {
    throw new Error('未找到预期的 scoped SCSS 样式块');
  }
  const result = compileStyle({
    source: style.content,
    filename,
    id: 'data-v-test',
    scoped: true,
    preprocessLang: style.lang,
  });

  expect(result.errors).toEqual([]);
  // 使用自定义 Select 组件替代原生 select，在所有平台统一外观
  const template = descriptor.template?.content ?? '';
  expect(template).toContain('<Select v-model=');
  // 确认不再使用原生 select 标签作为选择器
  const selectCount = (template.match(/<select\s/g) ?? []).length;
  expect(selectCount).toBe(0);
});

test('数字选项通过动态 value 保持 number 类型', () => {
  const { descriptor } = parse(readFileSync(filename, 'utf8'), { filename });
  const scriptContent = descriptor.script?.content ?? '';

  // 检查 musicQualityOptions computed 中包含预期的数字 value
  for (const value of [128000, 192000, 320000, 999000, 16, 22, 28, 36]) {
    expect(scriptContent).toContain(`value: ${value}`);
  }
  // 确认这些值通过 Select 组件的 options prop 传递，保持 number 类型
  expect(scriptContent).toContain('musicQualityOptions(): SelectOption[]');
});
