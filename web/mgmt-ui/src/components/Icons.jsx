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
  HiOutlineUsers,
  HiOutlineMail,
  HiOutlineFolder,
  HiOutlineShieldCheck,
  HiOutlineDocumentText,
  HiOutlineClock,
  HiOutlineTrendingUp,
  HiOutlineCollection,
  HiOutlineSearch,
} from 'react-icons/hi'

function RadioTowerIcon({ size = 18, className = '' }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={`icon ${className}`.trim()}
      aria-hidden
    >
      <path d="M4.9 16.1C1 12.2 1 5.8 4.9 1.9" />
      <path d="M7.8 4.7a6.14 6.14 0 0 0-.8 7.5" />
      <circle cx="12" cy="9" r="2" />
      <path d="M16.2 4.8c2 2 2.26 5.11.8 7.4" />
      <path d="M19.1 1.9a9.96 9.96 0 0 1 0 14.1" />
      <path d="M9.5 18h5" />
      <path d="m8 22 4-11 4 11" />
    </svg>
  )
}

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
  users: HiOutlineUsers,
  mail: HiOutlineMail,
  folder: HiOutlineFolder,
  shield: HiOutlineShieldCheck,
  logs: HiOutlineDocumentText,
  clock: HiOutlineClock,
  upgrade: HiOutlineTrendingUp,
  addons: HiOutlineCollection,
  search: HiOutlineSearch,
  radio: RadioTowerIcon,
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
