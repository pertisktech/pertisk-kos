import {
  HiOutlineViewGrid,
  HiOutlineStatusOnline,
  HiOutlineCloud,
  HiOutlineDesktopComputer,
  HiOutlineTemplate,
  HiOutlineClipboardList,
  HiOutlineCog,
  HiOutlineCube,
  HiOutlinePlus,
  HiOutlineTrash,
  HiOutlinePencil,
  HiOutlineCheck,
  HiOutlineX,
  HiOutlineDownload,
  HiOutlineUpload,
  HiOutlineExternalLink,
  HiOutlineSun,
  HiOutlineMoon,
  HiOutlineLogout,
  HiPlay,
  HiOutlineChip,
  HiOutlineViewBoards,
  HiOutlineDatabase,
  HiOutlineServer,
  HiOutlineGlobe,
  HiOutlineExclamation,
  HiOutlineChevronLeft,
  HiOutlineRefresh,
  HiOutlineChevronDown,
  HiOutlineMenu,
  HiOutlineChevronDoubleLeft,
  HiOutlineChevronDoubleRight,
  HiOutlineTerminal,
  HiOutlineClipboardCopy,
  HiOutlineUser,
  HiOutlineFolder,
  HiOutlineShieldCheck,
  HiOutlineDocumentText,
  HiOutlineClock,
  HiOutlineTrendingUp,
} from 'react-icons/hi'

const ICONS = {
  dashboard: HiOutlineViewGrid,
  clusters: HiOutlineStatusOnline,
  providers: HiOutlineCloud,
  machines: HiOutlineDesktopComputer,
  templates: HiOutlineTemplate,
  audit: HiOutlineClipboardList,
  settings: HiOutlineCog,
  packages: HiOutlineCube,
  plus: HiOutlinePlus,
  trash: HiOutlineTrash,
  edit: HiOutlinePencil,
  check: HiOutlineCheck,
  x: HiOutlineX,
  download: HiOutlineDownload,
  upload: HiOutlineUpload,
  external: HiOutlineExternalLink,
  sun: HiOutlineSun,
  moon: HiOutlineMoon,
  logout: HiOutlineLogout,
  play: HiPlay,
  cpu: HiOutlineChip,
  memory: HiOutlineViewBoards,
  disk: HiOutlineDatabase,
  worker: HiOutlineServer,
  network: HiOutlineGlobe,
  alert: HiOutlineExclamation,
  back: HiOutlineChevronLeft,
  reboot: HiOutlineRefresh,
  refresh: HiOutlineRefresh,
  'chevron-down': HiOutlineChevronDown,
  menu: HiOutlineMenu,
  'chevrons-left': HiOutlineChevronDoubleLeft,
  'chevrons-right': HiOutlineChevronDoubleRight,
  terminal: HiOutlineTerminal,
  copy: HiOutlineClipboardCopy,
  user: HiOutlineUser,
  folder: HiOutlineFolder,
  shield: HiOutlineShieldCheck,
  logs: HiOutlineDocumentText,
  clock: HiOutlineClock,
  upgrade: HiOutlineTrendingUp,
}

export function Icon({ name, size = 18, className = '' }) {
  const Cmp = ICONS[name]
  if (!Cmp) return null
  return <Cmp size={size} className={`icon ${className}`.trim()} aria-hidden />
}

export function Btn({ icon, children, variant = 'primary', className = '', ...rest }) {
  const v = variant === 'primary' ? '' : variant
  return (
    <button type="button" className={`btn-icon ${v} ${className}`.trim()} {...rest}>
      {icon && <Icon name={icon} size={16} />}
      {children && <span>{children}</span>}
    </button>
  )
}
