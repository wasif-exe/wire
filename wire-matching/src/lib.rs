use wire_exchange::order::{EventKind, MarketEvent, Side};
use wire_exchange::lob::Bbo;

pub const MAX_ORDERS: usize = 65536;
pub const PRICE_SLOTS: usize = 65536;
pub const NULL_IDX: u32 = u32::MAX;

const DEFAULT_EVENT: MarketEvent = MarketEvent {
    timestamp_ns: 0,
    order_id: 0,
    new_order_id: 0,
    price: 0,
    shares: 0,
    match_number: 0,
    symbol: [0; 8],
    kind: EventKind::None,
    side: Side::Buy,
    stock_locate: 0,
    _pad: [0; 12],
};

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct OrderSlot {
    pub price: u32,
    pub shares: u32,
    pub next: u32,
    pub generation: u16,
    pub side: u8,
    pub active: bool,
}

#[repr(C, align(32))]
#[derive(Clone, Copy)]
struct PriceLevel {
    total_shares: u64,
    order_count: u32,
    head: u32,
    tail: u32,
    active: bool,
    _pad: [u8; 7],
}

struct EngineData {
    orders: [OrderSlot; MAX_ORDERS],
    bid_levels: [PriceLevel; PRICE_SLOTS],
    ask_levels: [PriceLevel; PRICE_SLOTS],
}

pub struct MatchingEngine {
    data: Box<EngineData>,
    free_head: u32,
    best_bid: u32,
    min_bid: u32,
    best_ask: u32,
    max_ask: u32,
    match_number: u64,
    symbol: [u8; 8],
}

unsafe impl Send for MatchingEngine {}

#[repr(C)]
pub struct MatchResult {
    pub fill: MarketEvent,
    pub fill_count: u32,
    pub resting_order_id: u32,
    pub resting_generation: u16,
    _pad: u16,
}

impl MatchingEngine {
    pub fn new(symbol: [u8; 8]) -> Self {
        let layout = std::alloc::Layout::new::<EngineData>();
        // SAFETY: EngineData contains integer types and bools; zeroed memory is valid.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) } as *mut EngineData;
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        let data = unsafe { Box::from_raw(ptr) };
        let mut engine = Self {
            data,
            free_head: 0,
            best_bid: 0,
            min_bid: 0,
            best_ask: u32::MAX,
            max_ask: 0,
            match_number: 0,
            symbol,
        };
        let orders = engine.data.orders.as_mut_ptr();
        for i in 0..MAX_ORDERS {
            // SAFETY: i is within the allocated orders bounds.
            unsafe {
                (*orders.add(i)).next = if i + 1 < MAX_ORDERS { (i + 1) as u32 } else { NULL_IDX };
            }
        }
        let bids = engine.data.bid_levels.as_mut_ptr();
        let asks = engine.data.ask_levels.as_mut_ptr();
        for i in 0..PRICE_SLOTS {
            // SAFETY: i is within the allocated level bounds.
            unsafe {
                (*bids.add(i)).head = NULL_IDX;
                (*bids.add(i)).tail = NULL_IDX;
                (*asks.add(i)).head = NULL_IDX;
                (*asks.add(i)).tail = NULL_IDX;
            }
        }
        engine
    }

    #[inline(always)]
    fn alloc_order(&mut self) -> u32 {
        let id = self.free_head;
        if id != NULL_IDX {
            // SAFETY: free_head is within orders allocation.
            self.free_head = unsafe { (*self.data.orders.as_ptr().add(id as usize)).next };
        }
        id
    }

    #[inline(always)]
    pub fn match_order(
        &mut self,
        price: u32,
        shares: u32,
        side: Side,
        timestamp_ns: u64,
    ) -> MatchResult {
        let mut result = MatchResult {
            fill: DEFAULT_EVENT,
            fill_count: 0,
            resting_order_id: NULL_IDX,
            resting_generation: 0,
            _pad: 0,
        };

        let mut remaining = shares;
        let orders = self.data.orders.as_mut_ptr();

        if matches!(side, Side::Buy) {
            if self.best_ask <= price && self.best_ask != u32::MAX {
                let p_idx = (self.best_ask as usize) & (PRICE_SLOTS - 1);
                // SAFETY: p_idx is masked to PRICE_SLOTS bounds.
                let level = unsafe { &mut *self.data.ask_levels.as_mut_ptr().add(p_idx) };
                if level.active && level.head != NULL_IDX {
                    let head_idx = level.head as usize;
                    // SAFETY: head_idx is within orders allocation.
                    let order = unsafe { &mut *orders.add(head_idx) };
                    let fill_qty = if remaining < order.shares { remaining } else { order.shares };

                    self.match_number += 1;
                    result.fill.timestamp_ns = timestamp_ns;
                    result.fill.order_id = head_idx as u64;
                    result.fill.price = self.best_ask;
                    result.fill.shares = fill_qty;
                    result.fill.match_number = self.match_number;
                    result.fill.symbol = self.symbol;
                    result.fill.kind = EventKind::Trade;
                    result.fill.side = Side::Sell;
                    result.fill_count = 1;

                    remaining -= fill_qty;
                    order.shares -= fill_qty;
                    level.total_shares -= fill_qty as u64;

                    if order.shares == 0 {
                        let old_head = level.head;
                        level.head = order.next;
                        if level.head == NULL_IDX {
                            level.tail = NULL_IDX;
                        }
                        level.order_count -= 1;
                        order.active = false;
                        order.next = self.free_head;
                        self.free_head = old_head;
                        order.generation = order.generation.wrapping_add(1);
                        if level.order_count == 0 {
                            level.active = false;
                            self.advance_best_ask();
                        }
                    }
                }
            }
        } else {
            if self.best_bid >= price && self.best_bid != 0 {
                let p_idx = (self.best_bid as usize) & (PRICE_SLOTS - 1);
                // SAFETY: p_idx is masked to PRICE_SLOTS bounds.
                let level = unsafe { &mut *self.data.bid_levels.as_mut_ptr().add(p_idx) };
                if level.active && level.head != NULL_IDX {
                    let head_idx = level.head as usize;
                    // SAFETY: head_idx is within orders allocation.
                    let order = unsafe { &mut *orders.add(head_idx) };
                    let fill_qty = if remaining < order.shares { remaining } else { order.shares };

                    self.match_number += 1;
                    result.fill.timestamp_ns = timestamp_ns;
                    result.fill.order_id = head_idx as u64;
                    result.fill.price = self.best_bid;
                    result.fill.shares = fill_qty;
                    result.fill.match_number = self.match_number;
                    result.fill.symbol = self.symbol;
                    result.fill.kind = EventKind::Trade;
                    result.fill.side = Side::Buy;
                    result.fill_count = 1;

                    remaining -= fill_qty;
                    order.shares -= fill_qty;
                    level.total_shares -= fill_qty as u64;

                    if order.shares == 0 {
                        let old_head = level.head;
                        level.head = order.next;
                        if level.head == NULL_IDX {
                            level.tail = NULL_IDX;
                        }
                        level.order_count -= 1;
                        order.active = false;
                        order.next = self.free_head;
                        self.free_head = old_head;
                        order.generation = order.generation.wrapping_add(1);
                        if level.order_count == 0 {
                            level.active = false;
                            self.advance_best_bid();
                        }
                    }
                }
            }
        }

        if remaining > 0 {
            let oid = self.alloc_order();
            if oid != NULL_IDX {
                // SAFETY: oid is within orders allocation.
                let o = unsafe { &mut *orders.add(oid as usize) };
                o.price = price;
                o.shares = remaining;
                o.side = side as u8;
                o.active = true;
                o.next = NULL_IDX;
                let p_idx = (price as usize) & (PRICE_SLOTS - 1);
                if matches!(side, Side::Buy) {
                    // SAFETY: p_idx is masked to PRICE_SLOTS bounds.
                    let level = unsafe { &mut *self.data.bid_levels.as_mut_ptr().add(p_idx) };
                    if level.tail != NULL_IDX {
                        // SAFETY: level.tail is within orders allocation.
                        unsafe { (*orders.add(level.tail as usize)).next = oid; }
                    } else {
                        level.head = oid;
                    }
                    level.tail = oid;
                    level.order_count += 1;
                    level.total_shares += remaining as u64;
                    level.active = true;
                    if price > self.best_bid { self.best_bid = price; }
                    if price < self.min_bid || self.min_bid == 0 { self.min_bid = price; }
                } else {
                    // SAFETY: p_idx is masked to PRICE_SLOTS bounds.
                    let level = unsafe { &mut *self.data.ask_levels.as_mut_ptr().add(p_idx) };
                    if level.tail != NULL_IDX {
                        // SAFETY: level.tail is within orders allocation.
                        unsafe { (*orders.add(level.tail as usize)).next = oid; }
                    } else {
                        level.head = oid;
                    }
                    level.tail = oid;
                    level.order_count += 1;
                    level.total_shares += remaining as u64;
                    level.active = true;
                    if price < self.best_ask { self.best_ask = price; }
                    if price > self.max_ask { self.max_ask = price; }
                }
                result.resting_order_id = oid;
                // SAFETY: oid is within orders allocation.
                result.resting_generation = unsafe { (*orders.add(oid as usize)).generation };
            }
        }

        result
    }

    #[inline(always)]
    pub fn cancel_order(&mut self, order_id: u32, generation: u16) -> bool {
        if (order_id as usize) >= MAX_ORDERS { return false; }
        let orders = self.data.orders.as_mut_ptr();
        // SAFETY: order_id is bounds-checked above.
        let order = unsafe { &mut *orders.add(order_id as usize) };
        if !order.active || order.generation != generation { return false; }
        let price = order.price;
        let side = order.side;
        let shares = order.shares;
        let p_idx = (price as usize) & (PRICE_SLOTS - 1);
        if side == Side::Buy as u8 {
            // SAFETY: p_idx is masked to PRICE_SLOTS bounds.
            let level = unsafe { &mut *self.data.bid_levels.as_mut_ptr().add(p_idx) };
            Self::remove_from_level(level, orders, order_id);
            level.total_shares = level.total_shares.saturating_sub(shares as u64);
            if level.order_count == 0 {
                level.active = false;
                if price == self.best_bid { self.advance_best_bid(); }
            }
        } else {
            // SAFETY: p_idx is masked to PRICE_SLOTS bounds.
            let level = unsafe { &mut *self.data.ask_levels.as_mut_ptr().add(p_idx) };
            Self::remove_from_level(level, orders, order_id);
            level.total_shares = level.total_shares.saturating_sub(shares as u64);
            if level.order_count == 0 {
                level.active = false;
                if price == self.best_ask { self.advance_best_ask(); }
            }
        }
        order.active = false;
        order.next = self.free_head;
        self.free_head = order_id;
        order.generation = order.generation.wrapping_add(1);
        true
    }

    fn remove_from_level(level: &mut PriceLevel, orders: *mut OrderSlot, target: u32) {
        if level.head == target {
            // SAFETY: target is within orders allocation.
            level.head = unsafe { (*orders.add(target as usize)).next };
            if level.head == NULL_IDX { level.tail = NULL_IDX; }
            level.order_count -= 1;
            return;
        }
        let mut prev = level.head;
        while prev != NULL_IDX {
            // SAFETY: prev is within orders allocation.
            let next = unsafe { (*orders.add(prev as usize)).next };
            if next == target {
                // SAFETY: prev and target are within orders allocation.
                unsafe { (*orders.add(prev as usize)).next = (*orders.add(target as usize)).next; }
                if level.tail == target { level.tail = prev; }
                level.order_count -= 1;
                return;
            }
            prev = next;
        }
    }

    #[inline(always)]
    fn advance_best_ask(&mut self) {
        if self.best_ask >= self.max_ask || self.max_ask == 0 {
            self.best_ask = u32::MAX;
            self.max_ask = 0;
            return;
        }
        let mut p = self.best_ask + 1;
        while p <= self.max_ask {
            let idx = (p as usize) & (PRICE_SLOTS - 1);
            if self.data.ask_levels[idx].active { self.best_ask = p; return; }
            p += 1;
        }
        self.best_ask = u32::MAX;
        self.max_ask = 0;
    }

    #[inline(always)]
    fn advance_best_bid(&mut self) {
        if self.best_bid <= self.min_bid || self.best_bid == 0 {
            self.best_bid = 0;
            self.min_bid = 0;
            return;
        }
        let mut p = self.best_bid - 1;
        while p >= self.min_bid {
            let idx = (p as usize) & (PRICE_SLOTS - 1);
            if self.data.bid_levels[idx].active { self.best_bid = p; return; }
            if p == 0 { break; }
            p -= 1;
        }
        self.best_bid = 0;
        self.min_bid = 0;
    }

    #[inline(always)]
    pub fn bbo(&self) -> Bbo {
        let spread = if self.best_ask > self.best_bid && self.best_bid > 0 && self.best_ask < u32::MAX {
            self.best_ask - self.best_bid
        } else { 0 };
        Bbo {
            bid_price: self.best_bid,
            bid_shares: if self.best_bid > 0 {
                self.data.bid_levels[(self.best_bid as usize) & (PRICE_SLOTS - 1)].total_shares
            } else { 0 },
            ask_price: if self.best_ask < u32::MAX { self.best_ask } else { 0 },
            ask_shares: if self.best_ask < u32::MAX {
                self.data.ask_levels[(self.best_ask as usize) & (PRICE_SLOTS - 1)].total_shares
            } else { 0 },
            spread,
        }
    }

    #[inline(always)]
    pub fn order_active(&self, order_id: u32) -> bool {
        if (order_id as usize) >= MAX_ORDERS { return false; }
        self.data.orders[order_id as usize].active
    }

    #[inline(always)]
    pub fn order_generation(&self, order_id: u32) -> u16 {
        if (order_id as usize) >= MAX_ORDERS { return 0; }
        self.data.orders[order_id as usize].generation
    }

    #[inline(always)]
    pub fn order_shares(&self, order_id: u32) -> u32 {
        if (order_id as usize) >= MAX_ORDERS { return 0; }
        self.data.orders[order_id as usize].shares
    }

    #[inline(always)]
    pub fn orders_ptr(&self) -> *const OrderSlot { self.data.orders.as_ptr() }

    #[inline(always)]
    pub fn orders_ptr_mut(&mut self) -> *mut OrderSlot { self.data.orders.as_mut_ptr() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_buy_against_resting_ask() {
        let mut engine = MatchingEngine::new(*b"AAPL    ");
        let r1 = engine.match_order(18010, 200, Side::Sell, 1000);
        assert_eq!(r1.fill_count, 0);
        assert_ne!(r1.resting_order_id, NULL_IDX);
        let r2 = engine.match_order(18010, 100, Side::Buy, 2000);
        assert_eq!(r2.fill_count, 1);
        assert_eq!(r2.fill.price, 18010);
        assert_eq!(r2.fill.shares, 100);
        assert_eq!(r2.fill.kind, EventKind::Trade);
        assert_eq!(r2.resting_order_id, NULL_IDX);
        assert_eq!(engine.order_shares(r1.resting_order_id), 100);
    }

    #[test]
    fn match_sell_against_resting_bid() {
        let mut engine = MatchingEngine::new(*b"MSFT    ");
        let r1 = engine.match_order(18000, 300, Side::Buy, 1000);
        assert_eq!(r1.fill_count, 0);
        assert_ne!(r1.resting_order_id, NULL_IDX);
        let r2 = engine.match_order(18000, 150, Side::Sell, 2000);
        assert_eq!(r2.fill_count, 1);
        assert_eq!(r2.fill.price, 18000);
        assert_eq!(r2.fill.shares, 150);
        assert_eq!(engine.order_shares(r1.resting_order_id), 150);
    }

    #[test]
    fn cancel_with_aba_generation() {
        let mut engine = MatchingEngine::new(*b"TSLA    ");
        let r1 = engine.match_order(18000, 100, Side::Buy, 1000);
        let oid = r1.resting_order_id;
        let gen = r1.resting_generation;
        assert!(engine.cancel_order(oid, gen));
        assert!(!engine.order_active(oid));
        assert!(!engine.cancel_order(oid, gen));
    }

    #[test]
    fn bbo_tracking() {
        let mut engine = MatchingEngine::new(*b"AAPL    ");
        engine.match_order(18000, 100, Side::Buy, 1000);
        engine.match_order(18010, 200, Side::Sell, 2000);
        let bbo = engine.bbo();
        assert_eq!(bbo.bid_price, 18000);
        assert_eq!(bbo.ask_price, 18010);
        assert_eq!(bbo.spread, 10);
    }

    #[test]
    fn slot_alignment() {
        assert_eq!(std::mem::align_of::<OrderSlot>(), 16);
        assert_eq!(std::mem::size_of::<OrderSlot>(), 16);
        assert_eq!(std::mem::align_of::<PriceLevel>(), 32);
        assert_eq!(std::mem::size_of::<PriceLevel>(), 32);
    }
}
