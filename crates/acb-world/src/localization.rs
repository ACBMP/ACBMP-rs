//! Game text: `LocalizationPackage` -> `CompressedData`
//! (`CompressedLocalizationData`) -> `Data`, decoded by
//! [`anvil_forge::loc`]. `UIString.OasisLineID` is the string id.

use anvil_fastload::Value;
pub use anvil_forge::loc::LocString;

use crate::{Archive, Error};

impl Archive<'_> {
    /// The still-encoded `Data` bytes of the package named `package`
    /// (exact name first, else the first name containing it).
    pub fn localization_data(&self, package: &str) -> Result<Vec<u8>, Error> {
        let packages = self.uids_of("LocalizationPackage");
        let named = |f: &dyn Fn(&str) -> bool| packages.iter().copied().find(|u| self.loc(*u).is_some_and(|l| f(&l.name)));
        let uid = named(&|n| n == package)
            .or_else(|| named(&|n| n.contains(package)))
            .ok_or_else(|| Error::Missing(format!("no LocalizationPackage matching {package:?}")))?;
        let root = self.root(uid)?;
        let data = self
            .field(&root.obj, "CompressedData")
            .and_then(Value::as_obj)
            .and_then(|c| self.field(c, "Data"))
            .ok_or_else(|| Error::Missing(format!("{package}: no CompressedData.Data")))?;
        match data {
            Value::Packed { data, .. } | Value::Raw(data) => Ok(data.clone()),
            Value::Array(items) => Ok(items.iter().filter_map(Value::as_u32).map(|b| b as u8).collect()),
            _ => Err(Error::Missing(format!("{package}: unexpected Data value"))),
        }
    }

    /// Every string of the package, id order per section.
    pub fn localization(&self, package: &str) -> Result<Vec<LocString>, Error> {
        let bytes = self.localization_data(package)?;
        Ok(anvil_forge::loc::Localization::parse(&bytes)?.strings()?)
    }
}
