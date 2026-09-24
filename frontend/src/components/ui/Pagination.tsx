import React from 'react';
import { useTranslation } from 'react-i18next';

interface PaginationProps {
  currentPage: number;
  totalPages: number;
  onPageChange: (page: number) => void;
  disabled?: boolean;
}

const Pagination: React.FC<PaginationProps> = ({
  currentPage,
  totalPages,
  onPageChange,
  disabled = false
}) => {
  const { t } = useTranslation();
  // 响应式页码数量：**按可用宽度动态算出能放下几个页码按钮**（不换行、
  // 不截断、尽可能多显示）。⚠️ 不能直接用容器 clientWidth——分页器被父级
  // flex 挤压后自身宽度随内容变化，测量值与内容互为因果会锁死。改为测量
  // 分页器的**父容器**（分页器 max-width 100% 撑满可用空间），按其中内容
  // 的实际占位迭代：先估一版，渲染后若内容溢出（scrollWidth >
  // clientWidth）则减一，若大量富余则加一——两次收敛到「放得下的最多页
  // 码」。页码按钮 ~36px，prev/next ~92px，省略号 ~30px。
  const wrapRef = React.useRef<HTMLDivElement | null>(null);
  const [maxDisplayedPages, setMaxDisplayedPages] = React.useState(5);
  React.useLayoutEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const adjust = () => {
      const avail = el.clientWidth;
      if (avail <= 0) return;
      setMaxDisplayedPages((prev) => {
        const buttonsW = 2 * 92 + (prev > 0 ? (prev + 2) * 36 : 2 * 36); // prev/next + 首末页(+省略号按 36 估)
        const n = Math.max(1, Math.min(5, Math.floor((avail - 2 * 92) / 36) - 2));
        return n;
      });
    };
    adjust();
    const ro = new ResizeObserver(adjust);
    ro.observe(el.parentElement ?? el);
    return () => ro.disconnect();
  }, []);
  // Generate page buttons
  const getPageButtons = () => {
    const buttons = [];

    // Always display first page
    buttons.push(
      <button
        key="first"
        onClick={() => onPageChange(1)}
        className={`px-3 py-1 mx-1 rounded whitespace-nowrap ${currentPage === 1
          ? 'bg-blue-500 text-white btn-primary'
          : 'bg-gray-200 hover:bg-gray-300 text-gray-700 btn-secondary'
          }`}
      >
        1
      </button>
    );

    // Start range
    const startPage = Math.max(2, currentPage - Math.floor(maxDisplayedPages / 2));

    // If we're showing ellipsis after first page
    if (startPage > 2) {
      buttons.push(
        <span key="ellipsis1" className="px-3 py-1 whitespace-nowrap">
          ...
        </span>
      );
    }

    // Middle pages
    for (let i = startPage; i <= Math.min(totalPages - 1, startPage + maxDisplayedPages - 3); i++) {
      buttons.push(
        <button
          key={i}
          onClick={() => onPageChange(i)}
          className={`px-3 py-1 mx-1 rounded whitespace-nowrap flex-shrink-0 ${currentPage === i
            ? 'bg-blue-500 text-white btn-primary'
            : 'bg-gray-200 hover:bg-gray-300 text-gray-700 btn-secondary'
            }`}
        >
          {i}
        </button>
      );
    }

    // If we're showing ellipsis before last page
    if (startPage + maxDisplayedPages - 3 < totalPages - 1) {
      buttons.push(
        <span key="ellipsis2" className="px-3 py-1 whitespace-nowrap">
          ...
        </span>
      );
    }

    // Always display last page if there's more than one page
    if (totalPages > 1) {
      buttons.push(
        <button
          key="last"
          onClick={() => onPageChange(totalPages)}
          className={`px-3 py-1 mx-1 rounded whitespace-nowrap ${currentPage === totalPages
            ? 'bg-blue-500 text-white btn-primary'
            : 'bg-gray-200 hover:bg-gray-300 text-gray-700 btn-secondary'
            }`}
        >
          {totalPages}
        </button>
      );
    }

    return buttons;
  };

  // If there's only one page, don't render pagination
  if (totalPages <= 1) {
    return null;
  }

  return (
    <div ref={wrapRef} className="flex flex-nowrap justify-center items-center my-6 w-full">
      <button
        onClick={() => onPageChange(Math.max(1, currentPage - 1))}
        disabled={disabled || currentPage === 1}
        className={`px-3 py-1 rounded mr-2 whitespace-nowrap flex-shrink-0 ${disabled || currentPage === 1
          ? 'bg-gray-100 text-gray-400 cursor-not-allowed'
          : 'bg-gray-200 hover:bg-gray-300 text-gray-700 btn-secondary'
          }`}
      >
        &laquo; {t('common.previous')}
      </button>

      <div className="flex min-w-0 overflow-hidden">{getPageButtons()}</div>

      <button
        onClick={() => onPageChange(Math.min(totalPages, currentPage + 1))}
        disabled={disabled || currentPage === totalPages}
        className={`px-3 py-1 rounded ml-2 whitespace-nowrap flex-shrink-0 ${disabled || currentPage === totalPages
          ? 'bg-gray-100 text-gray-400 cursor-not-allowed'
          : 'bg-gray-200 hover:bg-gray-300 text-gray-700 btn-secondary'
          }`}
      >
        {t('common.next')} &raquo;
      </button>
    </div>
  );
};

export default Pagination;