
use cart_cache::*;
use dns;
use dns::{NormalizedQuestion, NormalizedQuestionKey, DNS_CLASS_IN, DNS_RCODE_NXDOMAIN};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use super::MAX_TTL;

#[derive(Clone, Debug)]
pub struct CacheEntry {
    pub expiration: Instant,
    pub packet: Vec<u8>,
}

impl CacheEntry {
    pub fn is_expired(&self) -> bool {
        let now = Instant::now();
        now > self.expiration
    }
}

#[derive(Clone)]
pub struct Cache {
    arc_mx: Arc<Mutex<CartCache<NormalizedQuestionKey, CacheEntry>>>,
    decrement_ttl: bool,
}

impl Cache {
    pub fn new(capacity: usize, decrement_ttl: bool) -> Cache {
        let arc = CartCache::new(capacity).unwrap();
        let arc_mx = Arc::new(Mutex::new(arc));
        Cache {
            arc_mx: arc_mx,
            decrement_ttl: decrement_ttl,
        }
    }

    pub fn frequent_recent_len(&self) -> (usize, usize) {
        let cache = self.arc_mx.lock().unwrap();
        (cache.len(), cache.len())
    }

    pub fn insert(&mut self,
                  normalized_question_key: NormalizedQuestionKey,
                  packet: Vec<u8>,
                  ttl: u32)
                  -> bool {
        debug_assert!(packet.len() >= dns::DNS_HEADER_SIZE);
        if packet.len() < dns::DNS_HEADER_SIZE {
            return false;
        }
        let now = Instant::now();
        let duration = Duration::from_secs(ttl as u64);
        let expiration = now + duration;
        let mut cache = self.arc_mx.lock().unwrap();
        let cache_entry = CacheEntry {
            expiration: expiration,
            packet: packet,
        };
        cache.insert(normalized_question_key, cache_entry)
    }

    pub fn get(&mut self, normalized_question_key: &NormalizedQuestionKey) -> Option<CacheEntry> {
        let mut cache = self.arc_mx.lock().unwrap();
        cache.get_mut(&normalized_question_key).and_then(|res| Some(res.clone()))
    }

    pub fn get2(&mut self, normalized_question: &NormalizedQuestion) -> Option<CacheEntry> {
        if let Some(special_packet) = self.handle_special_queries(normalized_question) {
            Some(CacheEntry {
                expiration: Instant::now() + Duration::from_secs(MAX_TTL as u64),
                packet: special_packet,
            })
        } else if normalized_question.qclass != DNS_CLASS_IN {
            Some(CacheEntry {
                expiration: Instant::now() + Duration::from_secs(MAX_TTL as u64),
                packet: dns::build_refused_packet(&normalized_question).unwrap(),
            })
        } else {
            let normalized_question_key = normalized_question.key();
            let cache_entry = self.get(&normalized_question_key);
            if let Some(mut cache_entry) = cache_entry {
                if self.decrement_ttl {
                    let now = Instant::now();
                    if now <= cache_entry.expiration {
                        let remaining_ttl = cache_entry.expiration.duration_since(now).as_secs();
                        let _ = dns::set_ttl(&mut cache_entry.packet, remaining_ttl as u32);
                    }
                }
                return Some(cache_entry);
            }
            if !normalized_question_key.dnssec {
                let qname = normalized_question_key.qname_lc;
                if let Some(qname_shifted) = dns::qname_shift(&qname) {
                    let mut normalized_question_key = normalized_question.key();
                    normalized_question_key.qname_lc = qname_shifted.to_owned();
                    let shifted_cache_entry = self.get(&normalized_question_key);
                    if let Some(shifted_cache_entry) = shifted_cache_entry {
                        debug!("Shifted query cached");
                        let shifted_packet = shifted_cache_entry.packet;
                        if shifted_packet.len() >= dns::DNS_HEADER_SIZE &&
                           dns::rcode(&shifted_packet) == DNS_RCODE_NXDOMAIN {
                            debug!("Shifted query returned NXDOMAIN");
                            return Some(CacheEntry {
                                expiration: shifted_cache_entry.expiration,
                                packet: dns::build_nxdomain_packet(&normalized_question).unwrap(),
                            });
                        }
                    }
                }
            }
            None
        }
    }

    fn handle_special_queries(&self, normalized_question: &NormalizedQuestion) -> Option<Vec<u8>> {
        if normalized_question.qclass == dns::DNS_CLASS_IN {
            if normalized_question.qtype == dns::DNS_TYPE_ANY {
                debug!("ANY query");
                let packet = dns::build_any_packet(&normalized_question).unwrap();
                return Some(packet);
            }
        }
        if normalized_question.qclass == dns::DNS_CLASS_CH &&
           normalized_question.qtype == dns::DNS_TYPE_TXT {
            debug!("CHAOS TXT");
            let packet = dns::build_version_packet(&normalized_question).unwrap();
            return Some(packet);
        }
        None
    }
}
