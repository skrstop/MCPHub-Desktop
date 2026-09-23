import React from 'react';

/**
 * Official Git logo (from simple-icons, viewBox 0 0 24 24).
 * Used wherever a *git-recognizable* mark is needed (RAG 导入弹框的 Git 数据源
 * 卡片/表单/徽章、文档列表/树形视图的 git 数据源标识) — the abstract lucide
 * `GitBranch` line-diagram reads as a random node-graph, not as "Git".
 *
 * Default color: the official Git orange (#f05032); pass an explicit
 * `style.color` to override, or spread extra styles freely (they merge).
 */
export const GitIcon: React.FC<React.SVGProps<SVGSVGElement>> = ({ style, ...props }) => (
  <svg
    role="img"
    viewBox="0 0 24 24"
    xmlns="http://www.w3.org/2000/svg"
    width={24}
    height={24}
    fill="currentColor"
    style={{ color: '#f05032', ...style }}
    {...props}
  >
    <title>Git</title>
    <path d="M13.09 23.549a1.54 1.54 0 0 1-2.18 0L.451 13.089a1.54 1.54 0 0 1 0-2.179l7.191-7.19 2.733 2.733a1.85 1.85 0 0 0 .964 2.326v6.66a1.849 1.849 0 1 0 1.54 0V8.957l2.508 2.508a1.85 1.85 0 1 0 1.09-1.09l-2.634-2.634a1.85 1.85 0 0 0-2.378-2.377L8.73 2.63 10.91.451a1.54 1.54 0 0 1 2.179 0l10.459 10.46a1.54 1.54 0 0 1 0 2.179z" />
  </svg>
);

export default GitIcon;
