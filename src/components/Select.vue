<template>
  <div ref="root" class="custom-select">
    <button
      ref="triggerEl"
      type="button"
      class="custom-select-trigger"
      :class="{ open, focused }"
      role="combobox"
      :aria-haspopup="'listbox'"
      :aria-expanded="open"
      :aria-controls="listboxId"
      :aria-activedescendant="open ? getOptionId(highlightIndex) : undefined"
      @click="toggle"
      @keydown="onTriggerKeydown"
      @focus="onFocus"
      @blur="onBlur"
    >
      <span class="value">{{ currentLabel }}</span>
      <span class="arrow">
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="M16.59 8.59 12 13.17 7.41 8.59 6 10l6 6 6-6z" />
        </svg>
      </span>
    </button>

    <Transition name="dropdown">
      <ul
        v-if="open"
        :id="listboxId"
        class="custom-select-menu"
        role="listbox"
        :aria-label="currentLabel || '选项列表'"
      >
        <li
          v-for="(opt, index) in options"
          :key="String(opt.value)"
          :id="getOptionId(index)"
          role="option"
          :class="{
            active: opt.value === modelValue,
            highlighted: index === highlightIndex,
          }"
          :aria-selected="opt.value === modelValue"
          @click="select(opt.value)"
          @mouseenter="highlightIndex = index"
        >
          {{ opt.label }}
        </li>
      </ul>
    </Transition>
  </div>
</template>

<script lang="ts">
import { defineComponent, type PropType } from 'vue';

export interface SelectOption {
  value: string | number | boolean;
  label: string;
}

/** Incremented counter to generate unique ARIA IDs per instance. */
let instanceCounter = 0;

/**
 * 自定义下拉选择框，替代原生 <select>。
 * 支持完整键盘导航和 ARIA 无障碍属性。
 */
export default defineComponent({
  name: 'CustomSelect',
  props: {
    /** v-model 绑定的值。 */
    modelValue: {
      type: [String, Number, Boolean] as PropType<string | number | boolean>,
      required: true,
    },
    /** 选项列表。 */
    options: {
      type: Array as PropType<SelectOption[]>,
      required: true,
    },
  },
  emits: ['update:modelValue'],
  data() {
    return {
      open: false,
      /** 当前键盘高亮的选项索引，-1 表示无高亮。 */
      highlightIndex: -1,
      focused: false,
      /** 唯一 ID 前缀，用于 ARIA 属性关联。 */
      idPrefix: `ypm-select-${++instanceCounter}`,
    };
  },
  computed: {
    /** 当前选中选项的标签文本。 */
    currentLabel(): string {
      const matched = this.options.find(opt => opt.value === this.modelValue);
      return matched?.label ?? '';
    },
    /** listbox 元素的唯一 ID。 */
    listboxId(): string {
      return `${this.idPrefix}-listbox`;
    },
  },
  watch: {
    /** 当选项列表变化时，同步高亮位置到当前选中项。 */
    options() {
      this.syncHighlightToValue();
    },
    modelValue() {
      this.syncHighlightToValue();
    },
  },
  methods: {
    /**
     * 生成指定索引选项的唯一 ID，用于 aria-activedescendant。
     * @param index 选项在数组中的索引
     * @returns 唯一 ID 字符串
     */
    getOptionId(index: number): string {
      return `${this.idPrefix}-opt-${index}`;
    },

    /** 将高亮索引同步到当前 modelValue 对应的选项。 */
    syncHighlightToValue() {
      const index = this.options.findIndex(
        opt => opt.value === this.modelValue
      );
      this.highlightIndex = index >= 0 ? index : -1;
    },

    /** 打开下拉菜单并聚焦到当前选中项。 */
    openMenu() {
      this.syncHighlightToValue();
      if (this.highlightIndex < 0) this.highlightIndex = 0;
      this.open = true;
      this.$nextTick(() => this.scrollHighlightIntoView());
    },

    /** 关闭下拉菜单并将焦点归还触发器。 */
    closeMenu() {
      if (!this.open) return;
      this.open = false;
      const trigger = this.$refs['triggerEl'] as HTMLElement | undefined;
      trigger?.focus();
    },

    /** 切换菜单展开状态。 */
    toggle() {
      this.open ? this.closeMenu() : this.openMenu();
    },

    /**
     * 选中指定值并关闭菜单。
     * @param value 要选中的值
     */
    select(value: string | number | boolean) {
      this.$emit('update:modelValue', value);
      this.closeMenu();
    },

    /** 高亮移动到指定索引，并在打开时滚动到可视区域。 */
    setHighlight(index: number) {
      if (index < 0 || index >= this.options.length) return;
      this.highlightIndex = index;
      if (this.open) this.$nextTick(() => this.scrollHighlightIntoView());
    },

    /** 将高亮选项滚动到视口内。 */
    scrollHighlightIntoView() {
      if (this.highlightIndex < 0 || !this.open) return;
      const el = document.getElementById(this.getOptionId(this.highlightIndex));
      el?.scrollIntoView({ block: 'nearest' });
    },

    /**
     * 键盘事件统一处理。菜单关闭时打开菜单，菜单打开时导航/选择/关闭。
     * 所有按键走同一个入口，焦点始终在触发器上，避免事件丢失。
     * @param event 键盘事件
     */
    onTriggerKeydown(event: KeyboardEvent) {
      const key = event.key;
      const open = this.open;

      if (!open) {
        // 菜单关闭：打开菜单
        if (
          key === 'Enter' ||
          key === ' ' ||
          key === 'Spacebar' ||
          key === 'ArrowDown' ||
          key === 'Down' ||
          key === 'ArrowUp' ||
          key === 'Up'
        ) {
          event.preventDefault();
          this.openMenu();
        }
        return;
      }

      // 菜单打开：导航/选择/关闭
      switch (key) {
        case 'ArrowDown':
        case 'Down':
          event.preventDefault();
          this.setHighlight(
            this.highlightIndex < this.options.length - 1
              ? this.highlightIndex + 1
              : 0
          );
          break;
        case 'ArrowUp':
        case 'Up':
          event.preventDefault();
          this.setHighlight(
            this.highlightIndex > 0
              ? this.highlightIndex - 1
              : this.options.length - 1
          );
          break;
        case 'Home':
          event.preventDefault();
          this.setHighlight(0);
          break;
        case 'End':
          event.preventDefault();
          this.setHighlight(this.options.length - 1);
          break;
        case 'Enter':
        case ' ':
        case 'Spacebar': {
          const option = this.options[this.highlightIndex];
          if (option) {
            event.preventDefault();
            this.select(option.value);
          }
          break;
        }
        case 'Escape':
          event.preventDefault();
          this.closeMenu();
          break;
        case 'Tab':
          this.closeMenu();
          break;
      }
    },

    /** 文档点击外部时关闭菜单。 */
    onDocumentClick(event: MouseEvent) {
      const root = this.$refs['root'] as HTMLElement | undefined;
      if (root && !root.contains(event.target as Node)) {
        this.open = false;
      }
    },

    /** 触发器获取焦点时的状态标记。 */
    onFocus() {
      this.focused = true;
    },

    /** 触发器失去焦点时的状态标记。 */
    onBlur() {
      this.focused = false;
    },
  },
  mounted() {
    document.addEventListener('click', this.onDocumentClick);
    this.syncHighlightToValue();
  },
  unmounted() {
    document.removeEventListener('click', this.onDocumentClick);
  },
});
</script>

<style scoped lang="scss">
.custom-select {
  position: relative;
  display: inline-block;

  .custom-select-trigger {
    font-weight: 600;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    min-width: 192px;
    max-width: 600px;
    padding: 8px 12px;
    background: var(--color-secondary-bg);
    color: var(--color-text);
    border: none;
    border-radius: 8px;
    cursor: pointer;
    transition: background 0.2s ease, color 0.2s ease, box-shadow 0.2s ease;
    user-select: none;

    &:focus {
      outline: none;
    }

    &.focused,
    &:focus-visible {
      box-shadow: 0 0 0 2px var(--color-primary);
    }

    .value {
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }

    .arrow {
      display: flex;
      align-items: center;
      transition: transform 0.2s ease;
      opacity: 0.6;

      svg {
        width: 20px;
        height: 20px;
        fill: currentColor;
      }
    }

    &:hover {
      background: var(--color-secondary-bg);
      color: var(--color-primary);
    }

    &.open {
      color: var(--color-primary);
      background: var(--color-primary-bg);

      .arrow {
        transform: rotate(180deg);
      }
    }
  }

  .custom-select-menu {
    font-weight: 600;
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    width: 100%;
    box-sizing: border-box;
    list-style: none;
    margin: 0;
    padding: 4px;
    background: var(--color-body-bg);
    border: 1px solid rgba(128, 128, 128, 0.18);
    border-radius: 8px;
    // 深色模式下勾勒菜单边缘，浅色模式下几乎不可见
    box-shadow: 0 0 0 1px rgba(255, 255, 255, 0.1),
      0 6px 12px -4px rgba(0, 0, 0, 0.12);
    z-index: 100;
    overflow: hidden;
    max-height: 320px;
    overflow-y: auto;

    li {
      padding: 8px 12px;
      border-radius: 6px;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      font-size: 14px;
      cursor: pointer;
      color: var(--color-text);
      transition: background 0.15s ease, color 0.15s ease;

      &:hover,
      &.highlighted {
        background: rgba(128, 128, 128, 0.15);
      }

      &.active {
        color: var(--color-primary);
        font-weight: 600;
      }
    }
  }
}

// 展开：先加速后撞击减速并轻微回弹
.dropdown-enter-active {
  transition: opacity 0.2s ease, transform 0.3s cubic-bezier(0.34, 1.2, 0.64, 1);
  transform-origin: top;
}

// 收起：快速利落收回
.dropdown-leave-active {
  transition: opacity 0.15s ease, transform 0.15s ease;
  transform-origin: top;
}

.dropdown-enter-from,
.dropdown-leave-to {
  opacity: 0;
  transform: translateY(-4px) scale(0.98);
}
</style>
