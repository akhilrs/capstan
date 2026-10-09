//! The reviews (src/controller/reviews.ts).
//!
//! Every public function returns `Err(KernelError::Unported("reviews.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::dispatch::{arg, call, Args, ErrorHandler, RestartFilter};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::Value;

/// `checkReviewRequest`.
#[allow(unused_variables)]
pub fn check_review_request(
    kernel: &Kernel,
    subject_id: &str,
    reviewer_role: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported("reviews.checkReviewRequest".into()))
}

/// `beginReview`.
#[allow(unused_variables)]
pub fn begin_review(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("reviews.beginReview".into()))
}

/// `completeReview`.
#[allow(unused_variables)]
pub fn complete_review(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("reviews.completeReview".into()))
}

/// `unannouncedReviews`.
#[allow(unused_variables)]
pub fn unannounced_reviews(
    kernel: &Kernel,
    credential: &str,
    limit: Option<i64>,
) -> KernelResult<Value> {
    Err(KernelError::Unported("reviews.unannouncedReviews".into()))
}

/// `announceReview`.
#[allow(unused_variables)]
pub fn announce_review(
    kernel: &Kernel,
    context: &MutationContext,
    review_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported("reviews.announceReview".into()))
}

/// `reviews`.
#[allow(unused_variables)]
pub fn reviews(kernel: &Kernel, credential: &str, limit: Option<i64>) -> KernelResult<Value> {
    Err(KernelError::Unported("reviews.reviews".into()))
}

/// `reviewsToRelease`.
#[allow(unused_variables)]
pub fn reviews_to_release(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("reviews.reviewsToRelease".into()))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "checkReviewRequest" => {
            let subject_id = arg!(a.str(0, "subjectId"));
            let reviewer_role = arg!(a.str(1, "reviewerRole"));
            Some(call(check_review_request(
                kernel,
                subject_id,
                reviewer_role,
            )))
        }
        "beginReview" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(begin_review(kernel, &context, input)))
        }
        "completeReview" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(complete_review(kernel, &context, input)))
        }
        "unannouncedReviews" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(unannounced_reviews(kernel, credential, limit)))
        }
        "announceReview" => {
            let context = arg!(a.ctx(0));
            let review_id = arg!(a.str(1, "reviewId"));
            Some(call(announce_review(kernel, &context, review_id)))
        }
        "reviews" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(reviews(kernel, credential, limit)))
        }
        "reviewsToRelease" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(reviews_to_release(kernel, credential)))
        }
        _ => None,
    }
}
